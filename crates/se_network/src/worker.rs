//! Single-thread native ownership and panic-contained packet callbacks.

use std::ffi::c_void;
use std::io;
use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc::SyncSender};

use crate::config::{NatConfig, TransportProtocol};
use crate::ffi;
use crate::queue::MAX_FRAME_BYTES;
use crate::session::Shared;

struct Native(*mut c_void);
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            ffi::se_slirp_destroy(self.0);
        }
    }
}

pub(crate) fn run(
    config: NatConfig,
    wake: UdpSocket,
    shared: &Arc<Shared>,
    ready: SyncSender<Result<(), String>>,
) -> io::Result<()> {
    let subnet = config.validate().map_err(io::Error::other)?;
    let config_native = ffi::Config {
        network: u32::from(subnet.network()),
        mask: u32::from(subnet.mask()),
        gateway: u32::from(config.gateway),
        dns: u32::from(config.dns),
        dhcp_start: u32::from(config.dhcp_start),
    };
    #[cfg(windows)]
    let socket = {
        use std::os::windows::io::AsRawSocket;
        wake.as_raw_socket() as usize
    };
    #[cfg(unix)]
    let socket = {
        use std::os::fd::AsRawFd;
        wake.as_raw_fd() as usize
    };
    // The shared allocation outlives native cleanup; callbacks occur on this thread only.
    let native = Native(unsafe {
        ffi::se_slirp_create(
            &config_native,
            socket,
            packet,
            Arc::as_ptr(shared).cast_mut().cast(),
        )
    });
    if native.0.is_null() {
        let _ = ready.send(Err("libslirp initialization failed".into()));
        return Err(io::Error::other("libslirp initialization failed"));
    }
    for (index, rule) in config.forwards.iter().enumerate() {
        let result = unsafe {
            ffi::se_slirp_forward(
                native.0,
                i32::from(rule.protocol == TransportProtocol::Udp),
                u32::from(rule.host_address),
                rule.host_port,
                u32::from(rule.guest_address),
                rule.guest_port,
            )
        };
        if result != 0 {
            let message = format!(
                "Cannot bind NAT forward {} ({}:{})",
                index + 1,
                rule.host_address,
                rule.host_port
            );
            let _ = ready.send(Err(message.clone()));
            return Err(io::Error::other(message));
        }
    }
    if ready.send(Ok(())).is_err() {
        return Ok(());
    }
    while !shared.stopped.load(Ordering::Acquire) {
        let mut datagram = [0; 64];
        for _ in 0..64 {
            if wake.recv(&mut datagram).is_err() {
                break;
            }
        }
        for _ in 0..64 {
            let frame = shared.tx.lock().unwrap_or_else(|e| e.into_inner()).pop();
            let Some(frame) = frame else {
                break;
            };
            unsafe {
                ffi::se_slirp_input(native.0, frame.as_ptr(), frame.len());
            }
        }
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        let pending = !shared
            .tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty();
        let result = unsafe { ffi::se_slirp_poll(native.0, i32::from(pending)) };
        if result != 0 {
            return Err(io::Error::other(format!(
                "NAT socket polling failed (native error {result})"
            )));
        }
    }
    if let Some(failure) = shared.failure() {
        return Err(io::Error::other(failure));
    }
    Ok(())
}

unsafe extern "C" fn packet(bytes: *const u8, length: usize, opaque: *mut c_void) {
    let shared = unsafe { &*opaque.cast::<Shared>() };
    if length > MAX_FRAME_BYTES || bytes.is_null() {
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
        shared.push_received_frame(bytes);
    }));
    if result.is_err() {
        shared.latch_failure("NAT packet callback failed".into());
    }
}
