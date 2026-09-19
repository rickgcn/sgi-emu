//! NAT session lifetime, queue endpoints, and explicit worker wakeup.

use std::io;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use crate::config::NatConfig;
use crate::queue::FrameQueue;

const RECEIVE_PENDING: u8 = 1;
const FAILURE_PENDING: u8 = 2;

/// Host readiness is level-triggered and never becomes machine state.
/// Queue changes and their readiness updates share the receive mutex, so
/// clearing the last frame cannot hide a concurrent arrival. Atomic bit
/// updates preserve the independently published terminal failure.
#[derive(Default)]
pub(crate) struct Shared {
    pub tx: Mutex<FrameQueue>,
    rx: Mutex<FrameQueue>,
    failure: Mutex<Option<String>>,
    pending: AtomicU8,
    pub stopped: AtomicBool,
}

impl Shared {
    pub fn push_received_frame(&self, bytes: &[u8]) -> bool {
        let mut queue = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        let was_empty = queue.is_empty();
        let accepted = queue.push(bytes);
        if accepted && was_empty {
            self.pending.fetch_or(RECEIVE_PENDING, Ordering::Release);
        }
        accepted
    }

    fn try_receive_frame(&self) -> Option<Vec<u8>> {
        if self.pending.load(Ordering::Acquire) & RECEIVE_PENDING == 0 {
            return None;
        }
        let mut queue = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        let frame = queue.pop();
        if queue.is_empty() {
            self.pending.fetch_and(!RECEIVE_PENDING, Ordering::Release);
        }
        frame
    }

    /// Publishes the first terminal error and stops native processing.
    /// The detail remains available after runtime acknowledgement so callback
    /// failure survives native cleanup and the worker's exit notification.
    pub fn latch_failure(&self, reason: String) {
        let mut failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
        if failure.is_none() {
            *failure = Some(reason);
            self.pending.fetch_or(FAILURE_PENDING, Ordering::Release);
        }
        self.stopped.store(true, Ordering::Release);
    }

    pub fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn take_failure(&self) -> Option<String> {
        if self.pending.load(Ordering::Acquire) & FAILURE_PENDING == 0 {
            return None;
        }
        let failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
        let pending = self.pending.fetch_and(!FAILURE_PENDING, Ordering::AcqRel);
        if pending & FAILURE_PENDING != 0 {
            failure.clone()
        } else {
            None
        }
    }
}

/// A host-only NAT worker with two independently bounded FIFO queues.
///
/// Dropping the session wakes and joins its worker. No session state can be
/// serialized into a machine snapshot. Creating a session opens host sockets.
pub struct NetworkSession {
    shared: Arc<Shared>,
    wake: UdpSocket,
    worker: Option<JoinHandle<()>>,
}

impl NetworkSession {
    /// Creates libslirp on its owning thread and waits for initialization only.
    /// `failure_notify` runs once when that worker encounters a fatal error.
    ///
    /// # Errors
    /// Returns configuration, wakeup socket, thread, or native initialization errors.
    pub fn start(
        config: NatConfig,
        failure_notify: impl Fn(String) + Send + 'static,
    ) -> io::Result<Self> {
        config.validate().map_err(io::Error::other)?;
        let receiver = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        receiver.set_nonblocking(true)?;
        let wake = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        wake.connect(receiver.local_addr()?)?;
        wake.set_nonblocking(true)?;
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("se-network".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::worker::run(config, receiver, &worker_shared, ready_tx)
                }));
                let failure = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("NAT worker panicked".into()),
                };
                if let Some(failure) = failure {
                    worker_shared.latch_failure(failure);
                    failure_notify(
                        worker_shared
                            .failure()
                            .expect("terminal failure is retained"),
                    );
                }
            })?;
        let mut session = Self {
            shared,
            wake,
            worker: Some(worker),
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(session),
            Ok(Err(message)) => {
                session.shutdown();
                Err(io::Error::other(message))
            }
            Err(_) => {
                session.shutdown();
                Err(io::Error::other("NAT worker exited during initialization"))
            }
        }
    }

    /// Enqueues a frame without waiting for socket I/O. A full queue drops it.
    pub fn try_send_frame(&self, bytes: &[u8]) -> bool {
        if self.shared.stopped.load(Ordering::Acquire) {
            return false;
        }
        let mut queue = self.shared.tx.lock().unwrap_or_else(|e| e.into_inner());
        let wake = queue.is_empty();
        let accepted = queue.push(bytes);
        drop(queue);
        if accepted && wake {
            self.wakeup();
        }
        accepted
    }

    /// Reports queued input or an unacknowledged terminal failure without locking.
    /// A false result does not exclude an arrival after this observation; the
    /// runtime checks again at the next instruction boundary. Link readiness
    /// and machine filtering are deliberately not part of this host signal.
    #[must_use]
    #[inline]
    pub fn has_pending_work(&self) -> bool {
        self.shared.pending.load(Ordering::Acquire) != 0
    }

    /// Removes the oldest host frame; machine filtering has not happened yet.
    /// An empty queue returns without acquiring its mutex.
    pub fn try_receive_frame(&self) -> Option<Vec<u8>> {
        self.shared.try_receive_frame()
    }

    /// Takes the worker's terminal fault for the owning runtime to latch.
    /// Acknowledgement clears only its notification; the first error remains
    /// retained internally until the session is destroyed.
    pub fn take_failure(&self) -> Option<String> {
        self.shared.take_failure()
    }

    /// Wakes polling and joins the worker, releasing listeners and native state.
    pub fn shutdown(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.wakeup();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn wakeup(&self) {
        let _ = self.wake.send(&[1]);
    }
}

impl Drop for NetworkSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PortForwardRule, TransportProtocol};
    use std::fs;
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    const MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
    const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

    #[test]
    fn receive_readiness_tracks_bounded_fifo_contents() {
        let shared = Shared::default();
        assert!(shared.try_receive_frame().is_none());
        assert!(!shared.push_received_frame(&vec![0; crate::queue::MAX_FRAME_BYTES + 1]));
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
        for value in 0..=255_u8 {
            assert!(shared.push_received_frame(&[value]));
        }
        assert!(!shared.push_received_frame(&[99]));
        for value in 0..=255_u8 {
            assert_eq!(shared.pending.load(Ordering::Acquire), RECEIVE_PENDING);
            assert_eq!(shared.try_receive_frame(), Some(vec![value]));
        }
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
        assert!(shared.push_received_frame(&[42]));
        assert_eq!(shared.try_receive_frame(), Some(vec![42]));
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
    }

    #[test]
    fn consuming_the_last_frame_cannot_hide_a_concurrent_arrival() {
        let shared = Arc::new(Shared::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let producer_shared = Arc::clone(&shared);
        let producer_barrier = Arc::clone(&barrier);
        let producer = thread::spawn(move || {
            for _ in 0..2000 {
                producer_barrier.wait();
                assert!(producer_shared.push_received_frame(&[2]));
                producer_barrier.wait();
            }
        });
        for _ in 0..2000 {
            assert!(shared.push_received_frame(&[1]));
            barrier.wait();
            assert_eq!(shared.try_receive_frame(), Some(vec![1]));
            barrier.wait();
            assert_eq!(shared.pending.load(Ordering::Acquire), RECEIVE_PENDING);
            assert_eq!(shared.try_receive_frame(), Some(vec![2]));
            assert_eq!(shared.pending.load(Ordering::Acquire), 0);
        }
        producer.join().unwrap();
    }

    #[test]
    fn failure_and_receive_acknowledgements_preserve_each_other() {
        for receive_first in [false, true] {
            let shared = Shared::default();
            thread::scope(|scope| {
                scope.spawn(|| assert!(shared.push_received_frame(&[7])));
                scope.spawn(|| shared.latch_failure("first failure".into()));
            });
            assert_eq!(
                shared.pending.load(Ordering::Acquire),
                RECEIVE_PENDING | FAILURE_PENDING
            );
            if receive_first {
                assert_eq!(shared.try_receive_frame(), Some(vec![7]));
                assert_eq!(shared.pending.load(Ordering::Acquire), FAILURE_PENDING);
            }
            assert_eq!(shared.take_failure().as_deref(), Some("first failure"));
            shared.latch_failure("later failure".into());
            assert!(shared.take_failure().is_none());
            assert_eq!(shared.failure().as_deref(), Some("first failure"));
            if !receive_first {
                assert_eq!(shared.pending.load(Ordering::Acquire), RECEIVE_PENDING);
                assert_eq!(shared.try_receive_frame(), Some(vec![7]));
            }
            assert_eq!(shared.pending.load(Ordering::Acquire), 0);
            assert!(shared.stopped.load(Ordering::Acquire));
        }
    }

    #[test]
    fn idle_queries_do_not_wait_for_queue_or_failure_mutexes() {
        let session = Arc::new(NetworkSession::start(NatConfig::default(), |_| {}).unwrap());
        let queue_guard = session.shared.rx.lock().unwrap();
        let failure_guard = session.shared.failure.lock().unwrap();
        let reader_session = Arc::clone(&session);
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            sender
                .send((
                    reader_session.has_pending_work(),
                    reader_session.try_receive_frame(),
                    reader_session.take_failure(),
                ))
                .unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        drop(failure_guard);
        drop(queue_guard);
        reader.join().unwrap();
        assert_eq!(result.unwrap(), (false, None, None));
    }

    #[test]
    fn acknowledged_callback_failure_still_notifies_worker_exit_once() {
        let (sender, receiver) = mpsc::channel();
        let mut session = NetworkSession::start(NatConfig::default(), move |reason| {
            sender.send(reason).unwrap();
        })
        .unwrap();
        session.shared.latch_failure("callback failure".into());
        assert_eq!(session.take_failure().as_deref(), Some("callback failure"));
        session.wakeup();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            "callback failure"
        );
        session.shutdown();
        assert!(receiver.try_recv().is_err());
        assert!(!session.has_pending_work());
        assert!(session.take_failure().is_none());
    }

    fn learn_guest(session: &NetworkSession) {
        let mut arp = vec![0; 60];
        arp[..6].fill(255);
        arp[6..12].copy_from_slice(&MAC);
        arp[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
        arp[22..28].copy_from_slice(&MAC);
        arp[28..32].copy_from_slice(&GUEST_IP);
        arp[38..42].copy_from_slice(&GATEWAY_IP);
        assert!(session.try_send_frame(&arp));
        receive(session, |frame| {
            frame.len() >= 42 && frame[12..14] == [8, 6]
        });
    }

    fn checksum(bytes: &[u8]) -> u16 {
        let mut sum: u32 = bytes
            .chunks(2)
            .map(|word| (u32::from(word[0]) << 8) | u32::from(*word.get(1).unwrap_or(&0)))
            .sum();
        while sum > 65535 {
            sum = (sum & 65535) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn ipv4(protocol: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0; (34 + payload.len()).max(60)];
        frame[..6].copy_from_slice(&[0x52, 0x55, 10, 0, 2, 2]);
        frame[6..12].copy_from_slice(&MAC);
        frame[12..14].copy_from_slice(&[8, 0]);
        frame[14] = 0x45;
        frame[16..18].copy_from_slice(&((20 + payload.len()) as u16).to_be_bytes());
        frame[22] = 64;
        frame[23] = protocol;
        frame[26..30].copy_from_slice(&GUEST_IP);
        frame[30..34].copy_from_slice(&GATEWAY_IP);
        let sum = checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&sum.to_be_bytes());
        frame[34..34 + payload.len()].copy_from_slice(payload);
        frame
    }

    fn tcp(source: u16, destination: u16, sequence: u32, ack: u32, flags: u8) -> Vec<u8> {
        let mut segment = vec![0; 20];
        segment[..2].copy_from_slice(&source.to_be_bytes());
        segment[2..4].copy_from_slice(&destination.to_be_bytes());
        segment[4..8].copy_from_slice(&sequence.to_be_bytes());
        segment[8..12].copy_from_slice(&ack.to_be_bytes());
        segment[12] = 0x50;
        segment[13] = flags;
        segment[14..16].copy_from_slice(&32768u16.to_be_bytes());
        let mut pseudo = vec![10, 0, 2, 15, 10, 0, 2, 2, 0, 6, 0, 20];
        pseudo.extend(&segment);
        segment[16..18].copy_from_slice(&checksum(&pseudo).to_be_bytes());
        ipv4(6, &segment)
    }

    fn udp(source: u16, destination: u16, payload: &[u8]) -> Vec<u8> {
        let length = (8 + payload.len()) as u16;
        let mut datagram = vec![0; usize::from(length)];
        datagram[..2].copy_from_slice(&source.to_be_bytes());
        datagram[2..4].copy_from_slice(&destination.to_be_bytes());
        datagram[4..6].copy_from_slice(&length.to_be_bytes());
        datagram[8..].copy_from_slice(payload);
        let mut pseudo = GUEST_IP.to_vec();
        pseudo.extend_from_slice(&GATEWAY_IP);
        pseudo.extend_from_slice(&[0, 17]);
        pseudo.extend_from_slice(&length.to_be_bytes());
        pseudo.extend_from_slice(&datagram);
        datagram[6..8].copy_from_slice(&checksum(&pseudo).to_be_bytes());
        ipv4(17, &datagram)
    }

    /// Traditional BOOTP request broadcast from the unconfigured client 0.0.0.0:68.
    /// The request carries no DHCP magic cookie or DHCP options.
    fn bootp_request(xid: u32, mac: [u8; 6], vendor: &[u8]) -> Vec<u8> {
        let payload = 236 + vendor.len().max(64);
        let mut frame = vec![0; 14 + 20 + 8 + payload];
        frame[..6].fill(255);
        frame[6..12].copy_from_slice(&mac);
        frame[12..14].copy_from_slice(&[8, 0]);
        let ip = &mut frame[14..34];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&((28 + payload) as u16).to_be_bytes());
        ip[8] = 64;
        ip[9] = 17;
        ip[16..20].fill(255);
        let header = checksum(ip);
        ip[10..12].copy_from_slice(&header.to_be_bytes());
        frame[34..36].copy_from_slice(&68u16.to_be_bytes());
        frame[36..38].copy_from_slice(&67u16.to_be_bytes());
        frame[38..40].copy_from_slice(&((8 + payload) as u16).to_be_bytes());
        let bootp = &mut frame[42..];
        bootp[..4].copy_from_slice(&[1, 1, 6, 0]);
        bootp[4..8].copy_from_slice(&xid.to_be_bytes());
        bootp[10] = 0x80;
        bootp[28..34].copy_from_slice(&mac);
        bootp[236..236 + vendor.len()].copy_from_slice(vendor);
        frame
    }

    fn tftp_rrq(name: &str) -> Vec<u8> {
        let mut request = vec![0, 1];
        request.extend_from_slice(name.as_bytes());
        request.push(0);
        request.extend_from_slice(b"octet\0");
        request
    }

    fn tftp_ack(block: u16) -> Vec<u8> {
        let mut acknowledgement = vec![0, 4];
        acknowledgement.extend_from_slice(&block.to_be_bytes());
        acknowledgement
    }

    static NEXT_ROOT_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_root() -> PathBuf {
        let id = NEXT_ROOT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("sgi-emu-tftp-{}-{id}", std::process::id()))
    }

    #[test]
    fn native_udp_nat_and_forwarding_transfer_payloads() {
        let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let reserve = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let forwarded_port = reserve.local_addr().unwrap().port();
        drop(reserve);
        let mut config = NatConfig::default();
        config.forwards.push(PortForwardRule {
            protocol: TransportProtocol::Udp,
            host_address: Ipv4Addr::LOCALHOST,
            host_port: forwarded_port,
            guest_address: config.dhcp_start,
            guest_port: 9000,
        });
        let session = NetworkSession::start(config, |_| {}).unwrap();
        learn_guest(&session);
        let mut udp = vec![0; 12];
        udp[..2].copy_from_slice(&8000u16.to_be_bytes());
        udp[2..4].copy_from_slice(&peer.local_addr().unwrap().port().to_be_bytes());
        udp[4..6].copy_from_slice(&12u16.to_be_bytes());
        udp[8..].copy_from_slice(b"NAT!");
        assert!(session.try_send_frame(&ipv4(17, &udp)));
        let mut bytes = [0; 32];
        let (length, address) = peer.recv_from(&mut bytes).unwrap();
        assert_eq!(&bytes[..length], b"NAT!");
        peer.send_to(b"BACK", address).unwrap();
        let reply = receive(&session, |frame| {
            frame.len() >= 46 && frame[23] == 17 && frame[42..46] == *b"BACK"
        });
        assert_eq!(&reply[36..38], &8000u16.to_be_bytes());
        peer.send_to(b"FWD!", (Ipv4Addr::LOCALHOST, forwarded_port))
            .unwrap();
        let forwarded = receive(&session, |frame| {
            frame.len() >= 46 && frame[23] == 17 && frame[42..46] == *b"FWD!"
        });
        assert_eq!(&forwarded[36..38], &9000u16.to_be_bytes());
    }

    #[test]
    fn native_tcp_nat_and_forwarding_complete_handshakes() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let reserve = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let forwarded_port = reserve.local_addr().unwrap().port();
        drop(reserve);
        let mut config = NatConfig::default();
        config.forwards.push(PortForwardRule {
            protocol: TransportProtocol::Tcp,
            host_address: Ipv4Addr::LOCALHOST,
            host_port: forwarded_port,
            guest_address: config.dhcp_start,
            guest_port: 22,
        });
        let session = NetworkSession::start(config, |_| {}).unwrap();
        learn_guest(&session);
        let peer_port = listener.local_addr().unwrap().port();
        assert!(session.try_send_frame(&tcp(8000, peer_port, 100, 0, 2)));
        let reply = receive(&session, |frame| {
            frame.len() >= 54 && frame[23] == 6 && frame[47] & 0x12 == 0x12
        });
        let sequence = u32::from_be_bytes(reply[38..42].try_into().unwrap());
        assert!(session.try_send_frame(&tcp(8000, peer_port, 101, sequence.wrapping_add(1), 0x10)));
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        peer.write_all(b"TCP NAT!").unwrap();
        receive(&session, |frame| {
            frame.windows(8).any(|bytes| bytes == b"TCP NAT!")
        });
        let mut inbound = std::net::TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, forwarded_port).into(),
            Duration::from_secs(5),
        )
        .unwrap();
        let syn = receive(&session, |frame| {
            frame.len() >= 54 && frame[23] == 6 && frame[47] & 0x12 == 2
        });
        let port = u16::from_be_bytes(syn[34..36].try_into().unwrap());
        let sequence = u32::from_be_bytes(syn[38..42].try_into().unwrap());
        assert!(session.try_send_frame(&tcp(22, port, 200, sequence.wrapping_add(1), 0x12)));
        inbound
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        inbound.write_all(b"TCP FWD!").unwrap();
        receive(&session, |frame| {
            frame.windows(8).any(|bytes| bytes == b"TCP FWD!")
        });
    }

    fn receive(session: &NetworkSession, predicate: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(frame) = session.try_receive_frame()
                && predicate(&frame)
            {
                return frame;
            }
            assert!(
                Instant::now() < deadline,
                "no expected NAT response; fault: {:?}",
                session.take_failure()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn native_session_answers_gateway_arp_and_releases_listeners() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut config = NatConfig::default();
        config.forwards.push(PortForwardRule {
            protocol: TransportProtocol::Tcp,
            host_address: Ipv4Addr::LOCALHOST,
            host_port: port,
            guest_address: config.dhcp_start,
            guest_port: 22,
        });
        let mut session = NetworkSession::start(config, |error| panic!("{error}")).unwrap();
        let mac = [2, 0, 0, 0, 0, 1];
        let mut arp = vec![0; 60];
        arp[..6].fill(255);
        arp[6..12].copy_from_slice(&mac);
        arp[12..22].copy_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
        arp[22..28].copy_from_slice(&mac);
        arp[28..32].copy_from_slice(&[10, 0, 2, 15]);
        arp[38..42].copy_from_slice(&[10, 0, 2, 2]);
        assert!(session.try_send_frame(&arp));
        let reply = receive(&session, |frame| {
            frame.len() >= 42 && frame[12..14] == [8, 6]
        });
        assert_eq!(&reply[..6], &mac);
        assert_eq!(&reply[20..22], &[0, 2]);
        assert_eq!(&reply[28..32], &[10, 0, 2, 2]);
        session.shutdown();
        let _rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    }

    #[test]
    fn failed_forward_binding_releases_previously_created_listeners() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let free_port = free.local_addr().unwrap().port();
        drop(free);
        let mut config = NatConfig::default();
        for port in [free_port, occupied.local_addr().unwrap().port()] {
            config.forwards.push(PortForwardRule {
                protocol: TransportProtocol::Tcp,
                host_address: Ipv4Addr::LOCALHOST,
                host_port: port,
                guest_address: config.dhcp_start,
                guest_port: 22,
            });
        }
        assert!(NetworkSession::start(config, |_| {}).is_err());
        let _rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, free_port)).unwrap();
    }

    fn isolated_subnet_config() -> NatConfig {
        NatConfig {
            subnet: "192.168.73.0/24".into(),
            gateway: Ipv4Addr::new(192, 168, 73, 2),
            dns: Ipv4Addr::new(192, 168, 73, 3),
            dhcp_start: Ipv4Addr::new(192, 168, 73, 40),
            forwards: Vec::new(),
            tftp_root: None,
            bootfile: None,
        }
    }

    #[test]
    fn native_dhcp_uses_configured_subnet_and_resolvers() {
        let session = NetworkSession::start(isolated_subnet_config(), |_| {}).unwrap();
        let frame = bootp_request(
            0x0102_0304,
            [2, 0, 0, 0, 0, 2],
            &[99, 130, 83, 99, 53, 1, 1, 55, 1, 6, 255],
        );
        assert!(session.try_send_frame(&frame));
        let reply = receive(&session, |frame| {
            frame.len() > 282 && frame[12..14] == [8, 0] && frame[23] == 17
        });
        assert_eq!(&reply[58..62], &[192, 168, 73, 40]);
        assert!(
            reply[282..]
                .windows(6)
                .any(|option| option == [6, 4, 192, 168, 73, 3])
        );
        assert!(
            reply[282..]
                .windows(6)
                .any(|option| option == [3, 4, 192, 168, 73, 2])
        );
    }

    #[test]
    fn native_bootp_answers_legacy_clients() {
        let session = NetworkSession::start(isolated_subnet_config(), |_| {}).unwrap();
        let mac = [2, 0, 0, 0, 0, 2];
        assert!(session.try_send_frame(&bootp_request(0x0bad_cafe, mac, &[])));
        let reply = receive(&session, |frame| {
            frame.len() > 282 && frame[23] == 17 && frame[42] == 2
        });
        assert_eq!(&reply[..6], &[255; 6]);
        assert_eq!(&reply[26..30], &[192, 168, 73, 2]);
        assert_eq!(&reply[30..34], &[255; 4]);
        assert_eq!(&reply[34..36], &67u16.to_be_bytes());
        assert_eq!(&reply[36..38], &68u16.to_be_bytes());
        assert_eq!(&reply[42..50], &[2, 1, 6, 0, 0x0b, 0xad, 0xca, 0xfe]);
        assert_eq!(&reply[58..62], &[192, 168, 73, 40]);
        assert_eq!(&reply[62..66], &[192, 168, 73, 2]);
        assert_eq!(&reply[70..76], &mac);
        assert!(reply[86..278].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn native_bootp_reply_carries_the_configured_bootfile() {
        let config = NatConfig {
            bootfile: Some("stand/sa".into()),
            ..NatConfig::default()
        };
        let session = NetworkSession::start(config, |_| {}).unwrap();
        assert!(session.try_send_frame(&bootp_request(0x1234_5678, MAC, &[])));
        let reply = receive(&session, |frame| {
            frame.len() > 282 && frame[23] == 17 && frame[42] == 2
        });
        assert_eq!(&reply[150..158], b"stand/sa");
        assert!(reply[158..278].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn native_tftp_serves_files_from_the_configured_root() {
        let content = b"sgi-emu tftp integration test";
        let root = temporary_root();
        fs::create_dir_all(root.join("stand")).unwrap();
        fs::write(root.join("stand/sa"), content).unwrap();
        let config = NatConfig {
            tftp_root: Some(root.to_str().unwrap().into()),
            ..NatConfig::default()
        };
        let session = NetworkSession::start(config, |_| {}).unwrap();
        learn_guest(&session);
        assert!(session.try_send_frame(&udp(2000, 69, &tftp_rrq("stand/sa"))));
        let reply = receive(&session, |frame| {
            frame.len() >= 46 && frame[23] == 17 && frame[42..44] == [0, 3]
        });
        assert_eq!(&reply[..6], &MAC);
        assert_eq!(&reply[6..12], &[0x52, 0x55, 10, 0, 2, 2]);
        assert_eq!(&reply[26..30], &GATEWAY_IP);
        assert_eq!(&reply[30..34], &GUEST_IP);
        assert_eq!(&reply[34..36], &69u16.to_be_bytes());
        assert_eq!(&reply[36..38], &2000u16.to_be_bytes());
        assert_eq!(&reply[44..46], &[0, 1]);
        assert_eq!(&reply[46..], content);
        drop(session);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn native_tftp_acknowledgements_continue_a_multi_block_transfer() {
        let content: Vec<u8> = (0..700).map(|index| (index % 251) as u8).collect();
        let root = temporary_root();
        fs::create_dir_all(root.join("stand")).unwrap();
        fs::write(root.join("stand/sa"), &content).unwrap();
        let config = NatConfig {
            tftp_root: Some(root.to_str().unwrap().into()),
            ..NatConfig::default()
        };
        let session = NetworkSession::start(config, |_| {}).unwrap();
        learn_guest(&session);
        assert!(session.try_send_frame(&udp(2000, 69, &tftp_rrq("stand/sa"))));
        let first = receive(&session, |frame| {
            frame.len() >= 46 && frame[23] == 17 && frame[42..44] == [0, 3]
        });
        assert_eq!(&first[34..36], &69u16.to_be_bytes());
        assert_eq!(&first[36..38], &2000u16.to_be_bytes());
        assert_eq!(&first[44..46], &[0, 1]);
        assert_eq!(first[46..].len(), 512);
        assert_eq!(&first[46..], &content[..512]);
        assert!(session.try_send_frame(&udp(2000, 69, &tftp_ack(1))));
        // Matching the block number keeps a repeated block 1 from satisfying
        // the wait for the acknowledged continuation.
        let second = receive(&session, |frame| {
            frame.len() >= 46 && frame[23] == 17 && frame[42..46] == [0, 3, 0, 2]
        });
        assert_eq!(&second[34..36], &69u16.to_be_bytes());
        assert_eq!(&second[36..38], &2000u16.to_be_bytes());
        assert_eq!(&second[44..46], &[0, 2]);
        assert!(second[46..].len() < 512);
        assert_eq!(&second[46..], &content[512..]);
        let mut transferred = first[46..].to_vec();
        transferred.extend_from_slice(&second[46..]);
        assert_eq!(transferred, content);
        drop(session);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn native_tftp_serves_files_from_a_unicode_root() {
        let content = b"sgi-emu unicode tftp integration test";
        let base = temporary_root();
        // Only the host root is non-ASCII. The guest requests the plain ASCII
        // TFTP file name `stand/sa`, so the case under test is the host
        // pathname encoding and never guest protocol encoding.
        let root = base.join("tftp-测试-日本語-é");
        assert!(!root.to_str().unwrap().is_ascii());
        fs::create_dir_all(root.join("stand")).unwrap();
        fs::write(root.join("stand/sa"), content).unwrap();
        let config = NatConfig {
            tftp_root: Some(root.to_str().unwrap().into()),
            ..NatConfig::default()
        };
        let session = NetworkSession::start(config, |_| {}).unwrap();
        learn_guest(&session);
        let request = tftp_rrq("stand/sa");
        assert!(request.is_ascii());
        assert!(session.try_send_frame(&udp(2000, 69, &request)));
        // Matching any TFTP reply first keeps a host that cannot address the
        // non-ASCII directory reporting the returned error instead of a timeout.
        let reply = receive(&session, |frame| {
            frame.len() >= 44 && frame[23] == 17 && frame[42..43] == [0]
        });
        assert_eq!(
            &reply[42..44],
            &[0, 3],
            "the built-in TFTP server must open a non-ASCII host root"
        );
        assert_eq!(&reply[46..], content);
        drop(session);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn native_tftp_is_disabled_without_a_configured_root() {
        let session = NetworkSession::start(NatConfig::default(), |_| {}).unwrap();
        learn_guest(&session);
        assert!(session.try_send_frame(&udp(2000, 69, &tftp_rrq("stand/sa"))));
        let reply = receive(&session, |frame| {
            frame.len() >= 63 && frame[23] == 17 && frame[42..44] == [0, 5]
        });
        assert_eq!(&reply[44..46], &[0, 2]);
        assert_eq!(&reply[46..63], b"Access violation\0");
    }
}
