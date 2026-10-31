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
fn exchange(
    version: &'static rustls::SupportedProtocolVersion,
    fragment: usize,
    trusted: bool,
    host: &str,
) -> Result<Vec<u8>, &'static str> {
    let provider = Arc::new(rustls_rustcrypto::provider());
    let mut roots = RootCertStore::empty();
    if trusted {
        roots
            .add(CertificateDer::from(
                include_bytes!("fixtures/tls/ca.der").to_vec(),
            ))
            .unwrap();
    }
    let config = ClientConfig::builder_with_details(provider.clone(), Arc::new(Clock))
        .with_protocol_versions(&[version])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connection = UnbufferedClientConnection::new(
        Arc::new(config),
        ServerName::try_from(host.to_owned()).unwrap(),
    )
    .unwrap();
    let request = b"GET /firmware.bin HTTP/1.1\r\nHost: updater.invalid\r\n\r\n";
    let mut client = Some(Engine::new(connection, request.to_vec()));
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
    let mut server = ServerConnection::new(Arc::new(server)).unwrap();
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
                Action::Plain => received.extend_from_slice(&engine.plain),
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
