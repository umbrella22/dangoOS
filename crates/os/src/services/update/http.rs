//! Streaming HTTP/1.1 framing over an already authenticated TLS connection.
use alloc::{string::String, vec::Vec};
use embedded_io_async::Read;

const HEADER_BYTES: usize = 16 * 1024;

pub fn connect_request(url: &super::Url) -> String {
    let endpoint = alloc::format!("{}:{}", url.host, url.port);
    alloc::format!("CONNECT {endpoint} HTTP/1.1\r\nHost: {endpoint}\r\n\r\n")
}

/// Read exactly the CONNECT header before starting TLS, without consuming any
/// tunnel bytes or accepting a proxy login page as an update response.
#[derive(Default)]
pub struct TunnelHead(Vec<u8>);
impl TunnelHead {
    pub fn push(&mut self, byte: u8) -> Result<bool, &'static str> {
        if self.0.len() == HEADER_BYTES {
            return Err("proxy header too large");
        }
        self.0.push(byte);
        if !self.0.ends_with(b"\r\n\r\n") {
            return Ok(false);
        }
        let first = self
            .0
            .split(|b| *b == b'\r')
            .next()
            .ok_or("proxy response")?;
        let line = core::str::from_utf8(first).map_err(|_| "proxy response")?;
        let mut words = line.splitn(3, ' ');
        if !matches!(words.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
            return Err("proxy HTTP version");
        }
        let code = words
            .next()
            .filter(|v| v.len() == 3)
            .ok_or("proxy status")?
            .parse::<u16>()
            .map_err(|_| "proxy status")?;
        match code {
            200..=299 => Ok(true),
            407 => Err("代理要求认证，请使用无认证 HTTP 代理"),
            _ => Err("代理拒绝连接，请检查地址和代理规则"),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Head {
    pub status: u16,
    pub length: Option<u64>,
    pub chunked: bool,
    pub location: Option<String>,
}
pub struct Response<R> {
    reader: R,
    buffer: [u8; 2048],
    position: usize,
    used: usize,
    remaining: Option<u64>,
    chunk: usize,
    chunk_end: bool,
    chunked: bool,
    finished: bool,
}
impl<R: Read> Response<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: [0; 2048],
            position: 0,
            used: 0,
            remaining: None,
            chunk: 0,
            chunk_end: false,
            chunked: false,
            finished: false,
        }
    }
    async fn byte(&mut self) -> Result<Option<u8>, &'static str> {
        if self.position == self.used {
            self.used = self
                .reader
                .read(&mut self.buffer)
                .await
                .map_err(|_| "HTTPS read")?;
            self.position = 0;
            if self.used == 0 {
                return Ok(None);
            }
        }
        let byte = self.buffer[self.position];
        self.position += 1;
        Ok(Some(byte))
    }
    async fn line(&mut self) -> Result<Vec<u8>, &'static str> {
        let mut line = Vec::new();
        loop {
            let b = self.byte().await?.ok_or("HTTP truncated line")?;
            if b == b'\n' {
                if line.pop() != Some(b'\r') {
                    return Err("HTTP line ending");
                }
                return Ok(line);
            }
            if line.len() >= HEADER_BYTES {
                return Err("HTTP header too large");
            }
            line.push(b);
        }
    }
    pub async fn head(&mut self) -> Result<Head, &'static str> {
        let first = self.line().await?;
        let status = core::str::from_utf8(&first).map_err(|_| "HTTP status")?;
        let mut parts = status.splitn(3, ' ');
        if !matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
            return Err("HTTP version");
        }
        let status = parts
            .next()
            .filter(|s| s.len() == 3)
            .ok_or("HTTP status")?
            .parse::<u16>()
            .map_err(|_| "HTTP status")?;
        let mut head = Head {
            status,
            length: None,
            chunked: false,
            location: None,
        };
        let mut bytes = first.len() + 2;
        loop {
            let line = self.line().await?;
            bytes += line.len() + 2;
            if bytes > HEADER_BYTES {
                return Err("HTTP headers too large");
            }
            if line.is_empty() {
                break;
            }
            let line = core::str::from_utf8(&line).map_err(|_| "HTTP header")?;
            let (name, value) = line.split_once(':').ok_or("HTTP header")?;
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                if head.length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err("HTTP content length");
                }
                head.length = Some(value.parse().map_err(|_| "HTTP content length")?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                if head.chunked || !value.eq_ignore_ascii_case("chunked") {
                    return Err("HTTP transfer encoding");
                }
                head.chunked = true;
            } else if name.eq_ignore_ascii_case("location") {
                if head.location.is_some() {
                    return Err("HTTP redirect");
                }
                head.location = Some(value.into());
            } else if name.eq_ignore_ascii_case("content-encoding")
                && !value.eq_ignore_ascii_case("identity")
            {
                return Err("HTTP compressed response");
            }
        }
        if head.chunked && head.length.is_some() {
            return Err("HTTP ambiguous body");
        }
        self.remaining = head.length;
        self.chunked = head.chunked;
        Ok(head)
    }
    pub async fn read(&mut self, out: &mut [u8]) -> Result<usize, &'static str> {
        if self.finished || out.is_empty() {
            return Ok(0);
        }
        if self.chunked && self.chunk == 0 {
            if self.chunk_end {
                if self.byte().await? != Some(b'\r') || self.byte().await? != Some(b'\n') {
                    return Err("HTTP chunk ending");
                }
            }
            let line = self.line().await?;
            let length = line.split(|b| *b == b';').next().unwrap();
            if length.is_empty() || !length.iter().all(u8::is_ascii_hexdigit) {
                return Err("HTTP chunk size");
            }
            self.chunk = usize::from_str_radix(core::str::from_utf8(length).unwrap(), 16)
                .map_err(|_| "HTTP chunk size")?;
            self.chunk_end = true;
            if self.chunk == 0 {
                let mut total = 0;
                loop {
                    let trailer = self.line().await?;
                    total += trailer.len() + 2;
                    if total > HEADER_BYTES {
                        return Err("HTTP trailers too large");
                    }
                    if trailer.is_empty() {
                        break;
                    }
                }
                self.finished = true;
                return Ok(0);
            }
        }
        let limit = if self.chunked {
            out.len().min(self.chunk)
        } else {
            self.remaining
                .map_or(out.len(), |n| (out.len() as u64).min(n) as usize)
        };
        if limit == 0 {
            self.finished = true;
            return Ok(0);
        }
        let mut written = 0;
        while written < limit {
            if self.position == self.used {
                if written > 0 {
                    break;
                }
                self.used = self
                    .reader
                    .read(&mut self.buffer)
                    .await
                    .map_err(|_| "HTTPS read")?;
                self.position = 0;
                if self.used == 0 {
                    if self.chunked || self.remaining.is_some_and(|n| n != 0) {
                        return Err("HTTP truncated body");
                    }
                    self.finished = true;
                    break;
                }
            }
            let take = (limit - written).min(self.used - self.position);
            out[written..written + take]
                .copy_from_slice(&self.buffer[self.position..self.position + take]);
            self.position += take;
            written += take;
        }
        if self.chunked {
            self.chunk -= written;
        } else if let Some(left) = &mut self.remaining {
            *left -= written as u64;
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connect_tunnels_use_proxy_dns_and_reject_login_or_refusal() {
        let url = super::super::Url::parse("https://github.com/a?token=a%2Bb").unwrap();
        assert_eq!(
            connect_request(&url),
            "CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443\r\n\r\n"
        );
        let mut head = TunnelHead::default();
        for (n, byte) in b"HTTP/1.1 200 Connection established\r\nX-Proxy: yes\r\n\r\n"
            .iter()
            .enumerate()
        {
            let done = head.push(*byte).unwrap();
            assert_eq!(
                done,
                n + 1 == b"HTTP/1.1 200 Connection established\r\nX-Proxy: yes\r\n\r\n".len()
            );
        }
        for response in [
            b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".as_slice(),
            b"HTTP/1.1 502 Bad Gateway\r\n\r\n".as_slice(),
            b"HTTP/1.1 2000 Invalid\r\n\r\n".as_slice(),
        ] {
            let mut head = TunnelHead::default();
            assert!(response
                .iter()
                .try_for_each(|b| head.push(*b).map(|_| ()))
                .is_err());
        }
    }
    struct Fragments<'a>(&'a [u8], usize);
    impl embedded_io::ErrorType for Fragments<'_> {
        type Error = core::convert::Infallible;
    }
    impl Read for Fragments<'_> {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
            let n = out.len().min(self.0.len()).min(self.1);
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }
    fn run<T>(f: impl core::future::Future<Output = T>) -> T {
        let mut f = core::pin::pin!(f);
        let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            core::task::Poll::Ready(v) => v,
            _ => panic!("memory reader yielded"),
        }
    }
    #[test]
    fn fragments_decode_fixed_and_chunked_bodies_and_preserve_redirect_queries() {
        for stride in [1, 7, 2048] {
            for wire in [b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".as_slice(),
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhe\r\n3;ext=yes\r\nllo\r\n0\r\nX-Trailer: yes\r\n\r\n".as_slice()] {
                run(async {
                    let mut response=Response::new(Fragments(wire,stride));
                    assert_eq!(response.head().await.unwrap().status,200);
                    let mut actual=Vec::new();let mut buf=[0;3];
                    loop {let n=response.read(&mut buf).await.unwrap();if n==0{break;}actual.extend_from_slice(&buf[..n]);}
                    assert_eq!(actual,b"hello");
                });
            }
        }
        run(async {
            let mut r=Response::new(Fragments(b"HTTP/1.1 302 Found\r\nLocation: https://assets.example/x?a=%2B\r\nContent-Length: 0\r\n\r\n",1));
            assert_eq!(
                r.head().await.unwrap().location.as_deref(),
                Some("https://assets.example/x?a=%2B")
            );
        });
    }
    #[test]
    fn ambiguous_lengths_and_incomplete_downloads_fail() {
        for wire in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n"
                .as_slice(),
        ] {
            assert!(run(async { Response::new(Fragments(wire, 1)).head().await }).is_err());
        }
        run(async {
            let mut response = Response::new(Fragments(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nabc",
                2,
            ));
            response.head().await.unwrap();
            let mut b = [0; 8];
            loop {
                match response.read(&mut b).await {
                    Ok(0) => panic!("accepted truncated image"),
                    Ok(_) => (),
                    Err(_) => break,
                }
            }
        });
    }
}
