//! One connection per turn. No automatic retry can duplicate a paid request.
use super::{
    config::Config,
    device,
    protocol::{self, Transcript},
    websocket::{self, Decoder, Event},
    Phase,
};
use crate::{
    formats::http::Response,
    platform::drivers::radio::https::{self, Tls},
    services::audio::capture,
};
use alloc::{string::String, vec, vec::Vec};
use embassy_net::Stack;
use embassy_time::{with_timeout, Duration, Instant};
use embedded_io_async::Read;

struct Ws<'a> {
    tls: Tls<'a>,
    decoder: Decoder,
    wire: Vec<u8>,
}
impl Ws<'_> {
    async fn send(&mut self, opcode: u8, payload: &[u8]) -> Result<(), &'static str> {
        let mut mask = [0; 4];
        getrandom::getrandom(&mut mask).map_err(|_| "语音随机数不可用")?;
        websocket::encode(opcode, payload, mask, &mut self.wire)?;
        with_timeout(Duration::from_secs(10), self.tls.write_plain(&self.wire))
            .await
            .map_err(|_| "语音发送超时")?
            .map_err(|_| "语音连接发送失败")
    }
    async fn receive(&mut self, transcript: &mut Transcript) -> Result<bool, &'static str> {
        let mut bytes = [0; 2048];
        let Some(n) = self
            .tls
            .poll_read(&mut bytes, Duration::from_millis(10))
            .await
            .map_err(|_| "转写连接读取失败")?
        else {
            return Ok(false);
        };
        if n == 0 {
            return Err("转写连接提前结束");
        }
        let mut changed = false;
        for byte in &bytes[..n] {
            match self.decoder.push(*byte)? {
                Some(Event::Text(text)) => changed |= transcript.event(text.as_bytes())?,
                Some(Event::Ping(payload)) => {
                    // Release the decoder's borrow before the await; control
                    // data never shares the accumulated transcript storage.
                    let mut ping = [0; 125];
                    let n = payload.len();
                    ping[..n].copy_from_slice(payload);
                    self.send(10, &ping[..n]).await?;
                }
                Some(Event::Close) if transcript.completed => return Ok(changed),
                Some(Event::Close) => return Err("转写服务关闭了连接"),
                Some(Event::Pong) | None => (),
            }
        }
        Ok(changed)
    }
}

#[inline(never)]
pub(super) async fn transcribe(
    stack: Stack<'static>,
    config: &Config,
    id: u32,
) -> Result<String, &'static str> {
    let mut rx = vec![0; 4096];
    let mut tx = vec![0; 4096];
    let mut nonce = [0; 16];
    getrandom::getrandom(&mut nonce).map_err(|_| "语音随机数不可用")?;
    let request = websocket::request(&config.asr, &nonce).into_bytes();
    let mut tls = https::connect(
        stack,
        &config.asr.url,
        config.proxy,
        &mut rx,
        &mut tx,
        request,
    )
    .await
    .map_err(|_| "无法连接转写服务，请检查地址或代理")?;
    with_timeout(Duration::from_secs(15), async {
        let mut upgrade = websocket::Upgrade::new(&nonce);
        let mut byte = [0];
        loop {
            if tls.read(&mut byte).await.map_err(|_| "转写 TLS 连接失败")? == 0 {
                return Err("转写握手提前结束");
            }
            if upgrade.push(byte[0])? {
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| "转写握手超时")??;
    let mut ws = Ws {
        tls,
        decoder: Decoder::default(),
        wire: Vec::with_capacity(6000),
    };
    ws.send(1, protocol::session_update(&config.asr.model).as_bytes())
        .await?;
    let mut transcript = Transcript::default();
    let mut packet = vec![0; super::capture::PACKET_BYTES];
    let mut json = String::with_capacity(5300);
    let mut committed_at = None;
    let connected = Instant::now();
    loop {
        let status = capture::status(id)?;
        if let Some(error) = status.error {
            return Err(error);
        }
        device::recorded(id, status.ms);
        if device::finish_requested() {
            capture::finish(id);
        }
        if status.finish || status.closed || device::finish_requested() {
            device::phase(id, Phase::Transcribing);
        }
        if transcript.updated {
            // Bound each pass so incoming transcript/error/ping events cannot
            // starve behind a full prebuffer.
            for _ in 0..4 {
                let status = capture::status(id)?;
                if status.bytes < packet.len() && !status.closed {
                    break;
                }
                let n = capture::pop(id, &mut packet)?;
                if n == 0 {
                    break;
                }
                protocol::append_audio(&packet[..n], &mut json);
                ws.send(1, json.as_bytes()).await?;
            }
            let status = capture::status(id)?;
            if status.drained && !transcript.sent_commit {
                if u64::from(status.ms) < super::MIN_RECORDING_MS {
                    return Err("录音太短，请说完后再按 AI 键");
                }
                transcript.sent_commit = true;
                ws.send(1, protocol::COMMIT.as_bytes()).await?;
                committed_at = Some(Instant::now());
                device::phase(id, Phase::Transcribing);
            }
        } else if connected.elapsed() > Duration::from_secs(10) {
            return Err("转写服务未确认会话配置");
        }
        if ws.receive(&mut transcript).await? {
            device::transcript(id, &transcript.text);
        }
        if transcript.completed {
            let _ = with_timeout(Duration::from_secs(1), ws.send(8, &1000u16.to_be_bytes())).await;
            return Ok(transcript.text);
        }
        if committed_at.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(45)) {
            return Err("等待转写结果超时");
        }
        embassy_futures::yield_now().await;
    }
}

#[inline(never)]
pub(super) async fn answer(
    stack: Stack<'static>,
    config: &Config,
    id: u32,
    text: &str,
) -> Result<(), &'static str> {
    use super::storage::device::{run, Job, Output};
    run(Job::Open(id, text.into())).await?;
    loop {
        answer_round(stack, config, id).await?;
        let mut called = false;
        loop {
            let Output::Tool(call) = run(Job::NextTool).await? else {
                unreachable!()
            };
            let Some(call) = call else {
                break;
            };
            let result = crate::apps::agent::execute(call.action).await;
            run(Job::ToolResult(call, result)).await?;
            called = true;
            embassy_futures::yield_now().await;
        }
        if !called {
            return Ok(());
        }
        run(Job::NextRound).await?;
    }
}
#[inline(never)]
async fn answer_round(stack: Stack<'static>, config: &Config, id: u32) -> Result<(), &'static str> {
    use super::storage::device::{run, Job, Output};
    let mut rx = vec![0; 4096];
    let mut tx = vec![0; 2048];
    let Output::Number(context_bytes) = run(Job::ContextBytes).await? else {
        unreachable!()
    };
    let prefix = protocol::responses::prefix(config);
    let suffix = protocol::responses::suffix();
    let body_bytes = prefix
        .len()
        .checked_add(context_bytes as usize)
        .and_then(|n| n.checked_add(suffix.len()))
        .ok_or("AI 上下文长度不可表示")?;
    let request = protocol::responses::head(config, body_bytes).into_bytes();
    let mut tls = https::connect(
        stack,
        &config.chat.url,
        config.proxy,
        &mut rx,
        &mut tx,
        request,
    )
    .await
    .map_err(|_| "无法连接 AI 服务，请检查地址或代理")?;
    tls.write_plain(prefix.as_bytes())
        .await
        .map_err(|_| "AI 请求发送失败")?;
    let mut bytes = vec![0; 2048];
    let mut offset = 0u32;
    while offset < context_bytes {
        let Output::Bytes(buffer, n) = run(Job::ContextRead(offset, bytes)).await? else {
            unreachable!()
        };
        bytes = buffer;
        if n == 0 || n as u32 > context_bytes - offset {
            return Err("AI 上下文读取长度无效");
        }
        tls.write_plain(&bytes[..n])
            .await
            .map_err(|_| "AI 上下文发送失败")?;
        offset += n as u32;
        embassy_futures::yield_now().await;
    }
    tls.write_plain(suffix.as_bytes())
        .await
        .map_err(|_| "AI 请求发送失败")?;
    let mut response = Response::new(tls);
    let head = response.head().await?;
    match head.status {
        200 => (),
        401 | 403 => return Err("AI 服务认证失败"),
        429 => return Err("AI 服务额度或速率受限"),
        _ => return Err("AI 服务请求失败"),
    }
    if !head
        .content_type
        .as_deref()
        .and_then(|s| s.split(';').next())
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("text/event-stream"))
    {
        return Err("AI 服务未返回 SSE 文字流");
    }
    loop {
        let n = response.read(&mut bytes).await?;
        if n == 0 {
            return Err("AI 回复流提前结束");
        }
        let Output::Block(buffer, done) = run(Job::Feed(id, bytes, n)).await? else {
            unreachable!()
        };
        bytes = buffer;
        if done {
            return Ok(());
        }
        embassy_futures::yield_now().await;
    }
}
