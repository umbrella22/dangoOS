//! Owned TLS record/crypto state. No socket or network Stack crosses threads.
use alloc::{vec, vec::Vec};
use rustls::{client::UnbufferedClientConnection, unbuffered::ConnectionState};

pub(super) struct Engine {
    connection: UnbufferedClientConnection,
    pub(super) incoming: Vec<u8>,
    pub(super) used: usize,
    pub(super) outgoing: Vec<u8>,
    pub(super) plain: Vec<u8>,
    pub(super) position: usize,
    request: Option<Vec<u8>>,
}
impl Engine {
    pub(super) fn new(connection: UnbufferedClientConnection, request: Vec<u8>) -> Self {
        Self {
            connection,
            incoming: vec![0; 18 * 1024],
            used: 0,
            outgoing: vec![0; 18 * 1024],
            plain: Vec::new(),
            position: 0,
            request: Some(request),
        }
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
                if let Some(request) = self.request.take() {
                    Action::Send(
                        traffic
                            .encrypt(&request, &mut self.outgoing)
                            .map_err(|_| "TLS request")?,
                    )
                } else {
                    Action::Receive
                }
            }
            ConnectionState::ReadTraffic(mut traffic) => {
                self.plain.clear();
                self.position = 0;
                while let Some(record) = traffic.next_record() {
                    let record = record.map_err(|_| "TLS record")?;
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
