//! Direction-neutral serial connections exposed by emulated machines.

/// A host-visible serial port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialPort {
    /// The first external serial port.
    A,
    /// The second external serial port.
    B,
}
