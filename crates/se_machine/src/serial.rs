//! Direction-neutral serial connections exposed by emulated machines.

use serde::{Deserialize, Serialize};

/// A host-visible serial port.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SerialPort {
    /// The first external serial port.
    A,
    /// The second external serial port.
    B,
}
