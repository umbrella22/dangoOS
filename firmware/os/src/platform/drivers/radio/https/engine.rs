//! Owned TLS record/crypto state. No socket or network Stack crosses threads.
use alloc::{sync::Arc, vec, vec::Vec};
use rustls::{client::UnbufferedClientConnection, unbuffered::ConnectionState};

// Every firmware client uses with_no_client_auth(). Keep all server certificate
// verification algorithms, while excluding an unused private-key loader vtable.
#[derive(Debug)]
struct NoClientKeys;
impl rustls::crypto::KeyProvider for NoClientKeys {
    fn load_private_key(
        &self,
        _: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
        Err(rustls::Error::General(
            "client certificates are not configured".into(),
        ))
    }
}
pub(super) fn provider() -> rustls::crypto::CryptoProvider {
    let mut provider = rustls_rustcrypto::provider();
    provider.key_provider = &NoClientKeys;
    provider
}

pub(super) struct Engine {
    connection: UnbufferedClientConnection,
    pub(super) incoming: Vec<u8>,
    pub(super) used: usize,
    pub(super) outgoing: Vec<u8>,
    pub(super) plain: Vec<u8>,
    pub(super) position: usize,
    request: Vec<u8>,
    pending: bool,
}
impl Engine {
    pub(super) fn new(connection: UnbufferedClientConnection, request: Vec<u8>) -> Self {
        Self {
            connection,
            incoming: vec![0; 18 * 1024],
            used: 0,
            outgoing: vec![0; 18 * 1024],
            plain: Vec::with_capacity(36 * 1024),
            position: 0,
            request,
            pending: true,
        }
    }
    /// Reuse the application's plaintext buffer for successive WS frames.
    pub(super) fn queue(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.pending || bytes.len() > 16 * 1024 {
            return Err("TLS write size or state");
        }
        self.request.clear();
        self.request.extend_from_slice(bytes);
        self.pending = true;
        Ok(())
    }
    pub(super) fn pending(&self) -> bool {
        self.pending
    }
    pub(super) fn step(&mut self) -> Result<Action, &'static str> {
        let status = self
            .connection
            .process_tls_records(&mut self.incoming[..self.used]);
        let mut discard = status.discard;
        let action = match status.state.map_err(|error| {
            log::warn!("update TLS: {error:?}");
            "HTTPS certificate or handshake"
        })? {
            ConnectionState::EncodeTlsData(mut data) => {
                Action::Send(data.encode(&mut self.outgoing).map_err(|_| "TLS encode")?)
            }
            ConnectionState::TransmitTlsData(data) => {
                data.done();
                Action::Continue
            }
            ConnectionState::BlockedHandshake => Action::Receive,
            ConnectionState::WriteTraffic(mut traffic) => {
                if self.pending {
                    self.pending = false;
                    Action::Send(
                        traffic
                            .encrypt(&self.request, &mut self.outgoing)
                            .map_err(|_| "TLS request")?,
                    )
                } else {
                    Action::Receive
                }
            }
            ConnectionState::ReadTraffic(mut traffic) => {
                self.plain.copy_within(self.position.., 0);
                self.plain.truncate(self.plain.len() - self.position);
                self.position = 0;
                while let Some(record) = traffic.next_record() {
                    let record = record.map_err(|_| "TLS record")?;
                    if self.plain.len() + record.payload.len() > 36 * 1024 {
                        return Err("TLS unread data limit");
                    }
                    self.plain.extend_from_slice(record.payload);
                    discard += record.discard;
                }
                if self.plain.is_empty() {
                    Action::Continue
                } else {
                    Action::Plain
                }
            }
            ConnectionState::PeerClosed | ConnectionState::Closed => Action::Closed,
            _ => return Err("TLS state"),
        };
        self.incoming.copy_within(discard..self.used, 0);
        self.used -= discard;
        Ok(action)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Send(usize),
    Receive,
    Continue,
    Plain,
    Closed,
}
