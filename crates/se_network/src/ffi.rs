//! Private C ABI; native sockets never cross this boundary as signed integers.

use std::ffi::{c_int, c_void};

#[repr(C)]
pub(crate) struct Config {
    pub network: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
    pub dhcp_start: u32,
}

unsafe extern "C" {
    pub(crate) fn se_slirp_create(
        config: *const Config,
        wake_socket: usize,
        packet: unsafe extern "C" fn(*const u8, usize, *mut c_void),
        opaque: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn se_slirp_destroy(session: *mut c_void);
    pub(crate) fn se_slirp_forward(
        session: *mut c_void,
        udp: c_int,
        host: u32,
        host_port: u16,
        guest: u32,
        guest_port: u16,
    ) -> c_int;
    pub(crate) fn se_slirp_input(session: *mut c_void, bytes: *const u8, length: usize);
    pub(crate) fn se_slirp_poll(session: *mut c_void, nonblocking: c_int) -> c_int;
}
