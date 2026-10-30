//! Exercise the firmware runner against embassy-net's actual DHCP socket.
#[path = "../src/tasks/radio/network.rs"]
mod network;

use embassy_net::driver::{Capabilities, Driver, HardwareAddress, LinkState, RxToken, TxToken};
use embassy_time::{Duration, MockDriver};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Wake, Waker},
};

const CLIENT: [u8; 6] = [2, 0, 0, 0, 0, 1];
const SERVER: [u8; 6] = [2, 0, 0, 0, 0, 2];
const ADDRESS: [u8; 4] = [192, 0, 2, 10];
const ROUTER: [u8; 4] = [192, 0, 2, 1];

struct State {
    link: LinkState,
    waker: Option<Waker>,
    incoming: VecDeque<Vec<u8>>,
    outgoing: Vec<Vec<u8>>,
    polls: usize,
}
#[derive(Clone)]
struct Peer(Arc<Mutex<State>>);
impl Peer {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            link: LinkState::Down,
            waker: None,
            incoming: VecDeque::new(),
            outgoing: Vec::new(),
            polls: 0,
        })))
    }
    fn change(&self, update: impl FnOnce(&mut State)) {
        let waker = {
            let mut state = self.0.lock().unwrap();
            update(&mut state);
            state.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    fn link(&self, link: LinkState) {
        self.change(|state| state.link = link);
    }
    fn receive(&self, frame: Vec<u8>) {
        self.change(|state| state.incoming.push_back(frame));
    }
    fn sent(&self, kind: u8) -> Vec<u8> {
        let frames = std::mem::take(&mut self.0.lock().unwrap().outgoing);
        frames
            .into_iter()
            .rev()
            .find(|frame| dhcp_type(frame) == Some(kind))
            .unwrap_or_else(|| panic!("missing DHCP message {kind}"))
    }
    fn polls(&self) -> usize {
        self.0.lock().unwrap().polls
    }
}
struct Rx(Vec<u8>);
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R {
        f(&mut self.0)
    }
}
struct Tx(Peer);
impl TxToken for Tx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0; len];
        let result = f(&mut frame);
        self.0 .0.lock().unwrap().outgoing.push(frame);
        result
    }
}
impl Driver for Peer {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx;
    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Rx, Tx)> {
        let mut state = self.0.lock().unwrap();
        state.waker = Some(cx.waker().clone());
        if state.link == LinkState::Down {
            return None;
        }
        state
            .incoming
            .pop_front()
            .map(|frame| (Rx(frame), Tx(self.clone())))
    }
    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Tx> {
        let mut state = self.0.lock().unwrap();
        state.waker = Some(cx.waker().clone());
        (state.link == LinkState::Up).then(|| Tx(self.clone()))
    }
    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        let mut state = self.0.lock().unwrap();
        state.waker = Some(cx.waker().clone());
        state.polls += 1;
        state.link
    }
    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = 1514;
        capabilities
    }
    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(CLIENT)
    }
}

struct Ready(AtomicBool);
impl Wake for Ready {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn drain<F: Future>(mut future: Pin<&mut F>, ready: &Arc<Ready>) {
    let waker = Waker::from(ready.clone());
    let mut cx = Context::from_waker(&waker);
    let mut polls = 0;
    while ready.0.swap(false, Ordering::SeqCst) {
        polls += 1;
        assert!(polls <= 16, "network runner remains continuously runnable");
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
}

fn dhcp_type(frame: &[u8]) -> Option<u8> {
    if frame.len() < 14 + 20 + 8 + 240 || frame[12..14] != [8, 0] {
        return None;
    }
    let ip = 14;
    let udp = ip + (frame[ip] as usize & 15) * 4;
    if frame[ip + 9] != 17 || frame[udp + 2..udp + 4] != 67u16.to_be_bytes() {
        return None;
    }
    let mut options = &frame[udp + 8 + 240..];
    while let Some((&code, remaining)) = options.split_first() {
        options = remaining;
        match code {
            0 => continue,
            255 => break,
            _ => {
                let (&len, remaining) = options.split_first()?;
                let (value, remaining) = remaining.split_at_checked(len as usize)?;
                if code == 53 {
                    return value.first().copied();
                }
                options = remaining;
            }
        }
    }
    None
}
fn dhcp_reply(request: &[u8], kind: u8) -> Vec<u8> {
    let mut payload = vec![0; 240];
    payload[0..3].copy_from_slice(&[2, 1, 6]);
    payload[4..8].copy_from_slice(&request[14 + 20 + 8 + 4..14 + 20 + 8 + 8]);
    payload[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    payload[16..20].copy_from_slice(&ADDRESS);
    payload[20..24].copy_from_slice(&ROUTER);
    payload[28..34].copy_from_slice(&CLIENT);
    payload[236..240].copy_from_slice(&[99, 130, 83, 99]);
    payload.extend_from_slice(&[53, 1, kind, 54, 4]);
    payload.extend_from_slice(&ROUTER);
    payload.extend_from_slice(&[51, 4, 0, 0, 14, 16, 1, 4, 255, 255, 255, 0, 3, 4]);
    payload.extend_from_slice(&ROUTER);
    payload.extend_from_slice(&[255]);
    let mut frame = vec![0; 14 + 20 + 8 + payload.len()];
    frame[..6].fill(255);
    frame[6..12].copy_from_slice(&SERVER);
    frame[12..14].copy_from_slice(&[8, 0]);
    let ip_len = (20 + 8 + payload.len()) as u16;
    let header = &mut frame[14..34];
    header[0] = 0x45;
    header[2..4].copy_from_slice(&ip_len.to_be_bytes());
    header[8] = 64;
    header[9] = 17;
    header[12..16].copy_from_slice(&ROUTER);
    header[16..20].fill(255);
    let mut sum = header
        .chunks_exact(2)
        .map(|word| u16::from_be_bytes([word[0], word[1]]) as u32)
        .sum::<u32>();
    while sum > 65535 {
        sum = (sum & 65535) + (sum >> 16);
    }
    header[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    frame[34..36].copy_from_slice(&67u16.to_be_bytes());
    frame[36..38].copy_from_slice(&68u16.to_be_bytes());
    frame[38..40].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    // An IPv4 UDP checksum of zero is permitted.
    frame[42..].copy_from_slice(&payload);
    frame
}

#[test]
fn offline_runner_parks_and_connected_dhcp_resumes_without_packet_delay() {
    let clock = MockDriver::get();
    clock.reset();
    let peer = Peer::new();
    let mut resources = embassy_net::StackResources::<4>::new();
    let (stack, runner) = embassy_net::new(
        peer.clone(),
        embassy_net::Config::dhcpv4(Default::default()),
        &mut resources,
        1,
    );
    let mut running = std::pin::pin!(network::run(stack, runner));
    let ready = Arc::new(Ready(AtomicBool::new(true)));
    drain(running.as_mut(), &ready);
    assert_eq!(peer.polls(), 1);
    assert!(!stack.is_link_up());

    // Spurious wakes while offline must not poll the due DHCP socket again.
    for _ in 0..128 {
        ready.wake_by_ref();
        drain(running.as_mut(), &ready);
    }
    assert_eq!(peer.polls(), 1);
    clock.advance(Duration::from_millis(49));
    drain(running.as_mut(), &ready);
    assert_eq!(peer.polls(), 1);
    clock.advance(Duration::from_millis(1));
    drain(running.as_mut(), &ready);
    assert_eq!(peer.polls(), 2);

    for _ in 0..2 {
        peer.link(LinkState::Up);
        drain(running.as_mut(), &ready);
        clock.advance(Duration::from_millis(50));
        drain(running.as_mut(), &ready);
        assert!(stack.is_link_up());
        let discover = peer.sent(1);
        // Offer/request/ack must finish without advancing the clock at all.
        let before = embassy_time::Instant::now();
        peer.receive(dhcp_reply(&discover, 2));
        drain(running.as_mut(), &ready);
        let request = peer.sent(3);
        peer.receive(dhcp_reply(&request, 5));
        drain(running.as_mut(), &ready);
        let config = stack.config_v4().expect("DHCP lease was not published");
        assert_eq!(config.address.address().octets(), ADDRESS);
        assert_eq!(config.gateway.unwrap().octets(), ROUTER);
        assert_eq!(embassy_time::Instant::now(), before);

        peer.link(LinkState::Down);
        drain(running.as_mut(), &ready);
        assert!(!stack.is_link_up());
        assert!(stack.config_v4().is_none(), "lost lease remains visible");
        let polls = peer.polls();
        for _ in 0..128 {
            ready.wake_by_ref();
            drain(running.as_mut(), &ready);
        }
        assert_eq!(peer.polls(), polls, "link-down runner did not park");
    }
}
