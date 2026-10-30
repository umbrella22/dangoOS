//! HTTPS transport; update policy, staging and activation remain in the task.
use alloc::{format, sync::Arc, vec};
use core::cell::Cell;
use embassy_net::{Stack, tcp::TcpSocket};
use embassy_time::{Duration, Instant, with_timeout};
use embedded_io::ErrorType;
use embedded_io_async::Read;
use os::update::{
    HttpProxy, Url,
    http::{Response, TunnelHead, connect_request},
};
use rustls::{
    ClientConfig, RootCertStore,
    client::UnbufferedClientConnection,
    pki_types::{ServerName, UnixTime},
    time_provider::TimeProvider,
};

static TIME: critical_section::Mutex<Cell<Option<(u64, u64)>>> =
    critical_section::Mutex::new(Cell::new(None));
#[derive(Debug)]
struct Clock;
impl TimeProvider for Clock {
    fn current_time(&self) -> Option<UnixTime> {
        critical_section::with(|cs| TIME.borrow(cs).get()).map(|(base, mono)| {
            UnixTime::since_unix_epoch(core::time::Duration::from_secs(
                base + Instant::now().as_secs().saturating_sub(mono),
            ))
        })
    }
}
fn entropy(out: &mut [u8]) -> Result<(), getrandom::Error> {
    let rng = esp_hal::rng::Rng::new();
    for chunk in out.chunks_mut(4) {
        chunk.copy_from_slice(&rng.random().to_le_bytes()[..chunk.len()]);
    }
    Ok(())
}
getrandom::register_custom_getrandom!(entropy);

/// Obtain wall time before certificate validation. NTP originate timestamps
/// bind replies to this request; certificate chain/name/time checks stay on.
pub async fn sync_clock(stack: Stack<'_>) -> Result<(), &'static str> {
    if critical_section::with(|cs| TIME.borrow(cs).get().is_some()) {
        return Ok(());
    }
    for host in ["time.cloudflare.com", "pool.ntp.org"] {
        let result = with_timeout(Duration::from_secs(5), async {
            let addresses = stack
                .dns_query(host, embassy_net::dns::DnsQueryType::A)
                .await
                .map_err(|_| "time DNS")?;
            let address = *addresses.first().ok_or("time address")?;
            let mut rx = [0; 512];
            let mut tx = [0; 64];
            let mut rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
            let mut tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 1];
            let mut socket = embassy_net::udp::UdpSocket::new(
                stack,
                &mut rx_meta,
                &mut rx,
                &mut tx_meta,
                &mut tx,
            );
            socket.bind(0).map_err(|_| "time socket")?;
            let mut query = [0; 48];
            query[0] = 0x23;
            entropy(&mut query[40..48]).map_err(|_| "time nonce")?;
            socket
                .send_to(&query, (address, 123))
                .await
                .map_err(|_| "time send")?;
            let mut reply = [0; 512];
            let (n, peer) = socket
                .recv_from(&mut reply)
                .await
                .map_err(|_| "time receive")?;
            if n < 48
                || peer.endpoint.addr != address
                || peer.endpoint.port != 123
                || reply[0] & 7 != 4
                || reply[0] >> 6 == 3
                || !(1..=15).contains(&reply[1])
                || reply[24..32] != query[40..48]
            {
                return Err("time response");
            }
            let ntp = u32::from_be_bytes(reply[40..44].try_into().unwrap()) as u64;
            let unix = ntp.checked_sub(2_208_988_800).ok_or("time epoch")?;
            let build = env!("DANGO_BUILD_UNIX")
                .parse::<u64>()
                .map_err(|_| "build time")?;
            if unix < build.saturating_sub(86400) {
                return Err("time predates build");
            }
            critical_section::with(|cs| {
                TIME.borrow(cs).set(Some((unix, Instant::now().as_secs())))
            });
            Ok(())
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            log::info!("update: SNTP clock synchronized");
            return Ok(());
        }
    }
    Err("无法同步时间，请稍后重试")
}

struct Tls<'a> {
    socket: TcpSocket<'a>,
    engine: Option<Engine>,
}
mod engine;
use engine::{Action, Engine};
fn engine(url: &Url) -> Result<Engine, &'static str> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let test_ca = include_bytes!(concat!(env!("OUT_DIR"), "/update-test-ca.der"));
    if !test_ca.is_empty() {
        roots
            .add(rustls::pki_types::CertificateDer::from(test_ca.as_slice()))
            .map_err(|_| "test CA")?;
    }
    let mut config = ClientConfig::builder_with_details(
        Arc::new(rustls_rustcrypto::provider()),
        Arc::new(Clock),
    )
    .with_safe_default_protocol_versions()
    .map_err(|_| "TLS versions")?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.resumption = rustls::client::Resumption::disabled();
    let name = ServerName::try_from(url.host.clone()).map_err(|_| "TLS hostname")?;
    let connection =
        UnbufferedClientConnection::new(Arc::new(config), name).map_err(|_| "TLS configuration")?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: dangoOS/{}\r\nAccept: application/octet-stream\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
        url.path,
        url.authority(),
        env!("CARGO_PKG_VERSION")
    );
    Ok(Engine::new(connection, request.into_bytes()))
}
impl<'a> Tls<'a> {
    async fn new(socket: TcpSocket<'a>, url: &Url) -> Result<Self, &'static str> {
        let url = url.clone();
        let engine = crate::service_executor::run(move || engine(&url)).await?;
        Ok(Self {
            socket,
            engine: Some(engine),
        })
    }
    async fn send(socket: &mut TcpSocket<'_>, mut bytes: &[u8]) -> Result<(), &'static str> {
        while !bytes.is_empty() {
            let n = socket.write(bytes).await.map_err(|_| "HTTPS send")?;
            if n == 0 {
                return Err("HTTPS send stalled");
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }
    async fn receive(&mut self) -> Result<bool, &'static str> {
        loop {
            let mut engine = self.engine.take().expect("one TLS step in flight");
            let (engine, action) = crate::service_executor::run(move || {
                let action = engine.step();
                (engine, action)
            })
            .await;
            self.engine = Some(engine);
            let engine = self.engine.as_mut().unwrap();
            match action? {
                Action::Send(n) => Self::send(&mut self.socket, &engine.outgoing[..n]).await?,
                Action::Receive => {
                    if engine.used == engine.incoming.len() {
                        return Err("TLS record too large");
                    }
                    let n = self
                        .socket
                        .read(&mut engine.incoming[engine.used..])
                        .await
                        .map_err(|_| "HTTPS receive")?;
                    if n == 0 {
                        return Ok(false);
                    }
                    engine.used += n;
                }
                Action::Continue => (),
                Action::Plain => return Ok(true),
                Action::Closed => return Ok(false),
            }
        }
    }
}
#[derive(Debug)]
struct Io;
impl core::fmt::Display for Io {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("HTTPS I/O")
    }
}
impl core::error::Error for Io {}
impl embedded_io::Error for Io {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}
impl ErrorType for Tls<'_> {
    type Error = Io;
}
impl Read for Tls<'_> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Io> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.engine.as_ref().unwrap().position == self.engine.as_ref().unwrap().plain.len() {
            let engine = self.engine.as_mut().unwrap();
            engine.plain.clear();
            engine.position = 0;
            if !self.receive().await.map_err(|error| {
                log::warn!("update HTTPS: {error}");
                Io
            })? {
                return Ok(0);
            }
        }
        let engine = self.engine.as_mut().unwrap();
        let n = out.len().min(engine.plain.len() - engine.position);
        out[..n].copy_from_slice(&engine.plain[engine.position..engine.position + n]);
        engine.position += n;
        Ok(n)
    }
}

pub async fn get(
    stack: Stack<'_>,
    url: &Url,
    proxy: Option<HttpProxy>,
    limit: u32,
    expected: Option<u32>,
    mut consume: impl AsyncFnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<u32, &'static str> {
    let mut url = url.clone();
    for _ in 0..6 {
        let (address, port) = if let Some(proxy) = proxy {
            (
                embassy_net::IpAddress::Ipv4(embassy_net::Ipv4Address::from(proxy.ip)),
                proxy.port,
            )
        } else {
            let addresses = with_timeout(
                Duration::from_secs(10),
                stack.dns_query(&url.host, embassy_net::dns::DnsQueryType::A),
            )
            .await
            .map_err(|_| "DNS timeout")?
            .map_err(|_| "DNS failed")?;
            (*addresses.first().ok_or("no IPv4 address")?, url.port)
        };
        let mut rx = vec![0; 4096];
        let mut tx = vec![0; 2048];
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(15)));
        with_timeout(Duration::from_secs(15), socket.connect((address, port)))
            .await
            .map_err(|_| {
                if proxy.is_some() {
                    "连接代理超时，请检查地址、端口和局域网访问"
                } else {
                    "TCP timeout"
                }
            })?
            .map_err(|_| {
                if proxy.is_some() {
                    "无法连接代理，请检查地址、端口和局域网访问"
                } else {
                    "TCP connect"
                }
            })?;
        if proxy.is_some() {
            with_timeout(Duration::from_secs(20), async {
                Tls::send(&mut socket, connect_request(&url).as_bytes()).await?;
                let mut head = TunnelHead::default();
                let mut byte = [0];
                loop {
                    if socket
                        .read(&mut byte)
                        .await
                        .map_err(|_| "代理连接读取失败")?
                        == 0
                    {
                        return Err("代理提前关闭连接");
                    }
                    if head.push(byte[0])? {
                        break;
                    }
                }
                Ok::<(), &'static str>(())
            })
            .await
            .map_err(|_| "代理响应超时")??;
        }
        let mut response = Response::new(Tls::new(socket, &url).await?);
        let head = with_timeout(Duration::from_secs(45), response.head())
            .await
            .map_err(|_| "HTTPS header timeout")??;
        if matches!(head.status, 301 | 302 | 303 | 307 | 308) {
            url = url.redirect(head.location.as_deref().ok_or("missing HTTPS redirect")?)?;
            continue;
        }
        if head.status != 200 {
            return Err("更新源暂时不可用，请稍后重试");
        }
        if head.length.is_some_and(|n| n > u64::from(limit))
            || expected
                .zip(head.length)
                .is_some_and(|(a, b)| u64::from(a) != b)
        {
            return Err("download length mismatch");
        }
        let mut buffer = [0; 4096];
        let mut total = 0u32;
        loop {
            let n = with_timeout(Duration::from_secs(20), response.read(&mut buffer))
                .await
                .map_err(|_| "download stalled")??;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u32)
                .filter(|n| *n <= limit)
                .ok_or("download too large")?;
            consume(&buffer[..n]).await?;
            embassy_futures::yield_now().await;
        }
        if expected.is_some_and(|n| n != total) {
            return Err("download truncated");
        }
        return Ok(total);
    }
    Err("too many HTTPS redirects")
}
