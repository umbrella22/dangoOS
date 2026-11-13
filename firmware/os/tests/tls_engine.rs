//! Real TLS records through the same owned engine used by firmware services.
extern crate alloc;
#[path = "../src/platform/drivers/radio/https/engine.rs"]
mod engine;
use engine::{Action, Engine};
use rustls::{
    client::UnbufferedClientConnection,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    time_provider::TimeProvider,
    ClientConfig, RootCertStore, ServerConfig, ServerConnection,
};
use std::{
    collections::VecDeque,
    io::{Cursor, Read, Write},
    sync::{mpsc, Arc},
    time::Duration,
};

// Public, disposable fixture key; never used by an update source. Certificates
// are valid from 2026-10-29 to 2036-10-26. Freeze verification within that range.
#[derive(Debug)]
struct Clock;
impl TimeProvider for Clock {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            1_793_404_800,
        )))
    }
}
fn pair(
    version: &'static rustls::SupportedProtocolVersion,
    trusted: bool,
    host: &str,
    request: &[u8],
) -> (Engine, ServerConnection) {
    let provider = Arc::new(rustls_rustcrypto::provider());
    let mut roots = RootCertStore::empty();
    if trusted {
        roots
            .add(CertificateDer::from(
                include_bytes!("fixtures/tls/ca.der").to_vec(),
            ))
            .unwrap();
    }
    let config = ClientConfig::builder_with_details(Arc::new(engine::provider()), Arc::new(Clock))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connection = UnbufferedClientConnection::new(
        Arc::new(config),
        ServerName::try_from(host.to_owned()).unwrap(),
    )
    .unwrap();
    let server = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(
                include_bytes!("fixtures/tls/server.der").to_vec(),
            )],
            PrivatePkcs8KeyDer::from(include_bytes!("fixtures/tls/server-key.der").to_vec()).into(),
        )
        .unwrap();
    let server = ServerConnection::new(Arc::new(server)).unwrap();
    (Engine::new(connection, request.to_vec()), server)
}
fn exchange(
    version: &'static rustls::SupportedProtocolVersion,
    fragment: usize,
    trusted: bool,
    host: &str,
) -> Result<Vec<u8>, &'static str> {
    let request = b"GET /firmware.bin HTTP/1.1\r\nHost: updater.invalid\r\n\r\n";
    let (client, mut server) = pair(version, trusted, host, request);
    let mut client = Some(client);
    let expected: Vec<u8> = (0..50_000).map(|n| (n % 251) as u8).collect();
    let mut incoming = VecDeque::new();
    let mut received_request = Vec::new();
    let mut received = Vec::new();
    let mut sent = false;
    let (jobs, worker_rx) = mpsc::channel::<Engine>();
    let (replies, main_rx) = mpsc::channel();
    // Move the production engine across a real thread boundary on every step.
    let worker = std::thread::spawn(move || {
        while let Ok(mut engine) = worker_rx.recv() {
            let result = engine.step();
            if replies.send((engine, result)).is_err() {
                break;
            }
        }
    });
    let result = (|| {
        for _ in 0..200_000 {
            jobs.send(client.take().unwrap()).unwrap();
            let (mut engine, action) = main_rx.recv().unwrap();
            match action? {
                Action::Send(n) => {
                    for part in engine.outgoing[..n].chunks(fragment) {
                        let mut bytes = Cursor::new(part);
                        server.read_tls(&mut bytes).unwrap();
                        server.process_new_packets().unwrap();
                    }
                }
                Action::Receive => {
                    if incoming.is_empty() {
                        return Err("TLS stalled");
                    }
                    let n = fragment
                        .min(incoming.len())
                        .min(engine.incoming.len() - engine.used);
                    assert!(n > 0, "record exceeds bounded receive buffer");
                    for i in 0..n {
                        engine.incoming[engine.used + i] = incoming.pop_front().unwrap();
                    }
                    engine.used += n;
                }
                Action::Plain => {
                    received.extend_from_slice(&engine.plain[engine.position..]);
                    engine.position = engine.plain.len();
                }
                Action::Continue => (),
                Action::Closed => return Ok(received),
            }
            // reader returns WouldBlock while the handshake/request is incomplete.
            let _ = server.reader().read_to_end(&mut received_request);
            if !sent && received_request.ends_with(b"\r\n\r\n") {
                assert_eq!(received_request, request);
                server.writer().write_all(&expected).unwrap();
                server.send_close_notify();
                sent = true;
            }
            let mut wire = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut wire).unwrap();
            }
            incoming.extend(wire);
            client = Some(engine);
        }
        Err("TLS failed to terminate")
    })();
    drop(jobs);
    worker.join().unwrap();
    if let Ok(ref bytes) = result {
        assert_eq!(bytes, &expected);
    }
    result
}
#[test]
fn tls13_fragmented_records_and_payload_across_service_thread() {
    assert!(exchange(&rustls::version::TLS13, 7, true, "updater.invalid").is_ok());
}
#[test]
fn tls12_single_byte_fragments_and_multiple_plaintext_records() {
    assert!(exchange(&rustls::version::TLS12, 1, true, "updater.invalid").is_ok());
}
#[test]
fn unknown_ca_and_wrong_hostname_are_rejected() {
    assert_eq!(
        exchange(&rustls::version::TLS13, 17, false, "updater.invalid"),
        Err("HTTPS certificate or handshake")
    );
    assert_eq!(
        exchange(&rustls::version::TLS13, 17, true, "other.invalid"),
        Err("HTTPS certificate or handshake")
    );
}

#[test]
fn repeated_client_writes_keep_unread_server_plaintext_in_tls12_and_tls13() {
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let first = b"first request";
        let second = vec![37; 16 * 1024];
        let (mut client, mut server) = pair(version, true, "updater.invalid", first);
        let expected = vec![91; 50_000];
        let mut received = Vec::new();
        let mut server_received = Vec::new();
        let mut incoming = VecDeque::new();
        let mut queued = false;
        let mut sent = false;
        for _ in 0..200_000 {
            match client.step().unwrap() {
                Action::Send(n) => {
                    for part in client.outgoing[..n].chunks(11) {
                        server.read_tls(&mut Cursor::new(part)).unwrap();
                        server.process_new_packets().unwrap();
                    }
                }
                Action::Receive => {
                    assert!(!incoming.is_empty(), "duplex TLS stalled");
                    let n = incoming
                        .len()
                        .min(13)
                        .min(client.incoming.len() - client.used);
                    for i in 0..n {
                        client.incoming[client.used + i] = incoming.pop_front().unwrap();
                    }
                    client.used += n;
                }
                Action::Plain => {
                    if !queued {
                        // Queue a second write while the first server record is
                        // still unread. The next record must retain those bytes.
                        client.queue(&second).unwrap();
                        queued = true;
                    } else {
                        received.extend_from_slice(&client.plain[client.position..]);
                        client.position = client.plain.len();
                    }
                }
                Action::Closed => break,
                Action::Continue => (),
            }
            let _ = server.reader().read_to_end(&mut server_received);
            if !sent && server_received == first {
                server.writer().write_all(&expected).unwrap();
                sent = true;
            }
            if server_received.len() == first.len() + second.len() {
                server.send_close_notify();
            }
            let mut wire = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut wire).unwrap();
            }
            incoming.extend(wire);
        }
        assert!(queued && !client.pending());
        assert_eq!(&server_received[..first.len()], first);
        assert_eq!(&server_received[first.len()..], second);
        assert_eq!(received, expected);
    }
}

#[test]
fn streamed_http_body_follows_the_pending_head_without_waiting_for_a_response() {
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let body = [b"{\"input\":[".as_slice(), &vec![b' '; 4096], b"]}"].concat();
        let head = format!(
            "POST /responses HTTP/1.1\r\nHost: updater.invalid\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let (mut client, mut server) = pair(version, true, "updater.invalid", head.as_bytes());
        assert!(client.pending());
        // Reproduces the initial state at connect(): a body cannot replace
        // the unsent HTTP head. The writer must finish that pending request.
        assert_eq!(client.queue(&body[..10]), Err("TLS write size or state"));
        let mut incoming = VecDeque::new();
        let mut request = Vec::new();
        let mut response = Vec::new();
        let mut offset = 0;
        let mut sent = false;
        let reply = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: done\n\n";
        for _ in 0..200_000 {
            if !client.pending() && offset < body.len() {
                let end = (offset + 2048).min(body.len());
                client.queue(&body[offset..end]).unwrap();
                offset = end;
            }
            match client.step().unwrap() {
                Action::Send(n) => {
                    server
                        .read_tls(&mut Cursor::new(&client.outgoing[..n]))
                        .unwrap();
                    server.process_new_packets().unwrap();
                }
                Action::Receive => {
                    assert!(
                        !incoming.is_empty(),
                        "request writer waited for a response before sending its body"
                    );
                    let n = incoming
                        .len()
                        .min(37)
                        .min(client.incoming.len() - client.used);
                    for _ in 0..n {
                        client.incoming[client.used] = incoming.pop_front().unwrap();
                        client.used += 1;
                    }
                }
                Action::Plain => {
                    response.extend_from_slice(&client.plain[client.position..]);
                    client.position = client.plain.len();
                }
                Action::Closed => break,
                Action::Continue => (),
            }
            let _ = server.reader().read_to_end(&mut request);
            if !sent && request.len() == head.len() + body.len() {
                assert_eq!(request, [head.as_bytes(), &body].concat());
                server.writer().write_all(reply).unwrap();
                server.send_close_notify();
                sent = true;
            }
            while server.wants_write() {
                let mut wire = Vec::new();
                server.write_tls(&mut wire).unwrap();
                incoming.extend(wire);
            }
        }
        assert!(sent);
        assert_eq!(response, reply);
    }
}
