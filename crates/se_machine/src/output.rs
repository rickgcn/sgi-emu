//! Frontend-visible output produced by an emulated machine.

use crate::serial::SerialPort;

/// Output accumulated during one machine time advancement.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct MachineOutput {
    serial_a: Vec<u8>,
    serial_b: Vec<u8>,
}

impl MachineOutput {
    /// Returns the bytes emitted by one external serial port.
    #[must_use]
    pub fn serial(&self, port: SerialPort) -> &[u8] {
        match port {
            SerialPort::A => &self.serial_a,
            SerialPort::B => &self.serial_b,
        }
    }

    /// Reports whether the machine produced no frontend-visible output.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.serial_a.is_empty() && self.serial_b.is_empty()
    }

    pub(crate) fn push_serial(&mut self, port: SerialPort, value: u8) {
        match port {
            SerialPort::A => self.serial_a.push(value),
            SerialPort::B => self.serial_b.push(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::serial::SerialPort;

    use super::MachineOutput;

    #[test]
    fn serial_ports_keep_independent_byte_order() {
        let mut output = MachineOutput::default();
        output.push_serial(SerialPort::B, 3);
        output.push_serial(SerialPort::A, 1);
        output.push_serial(SerialPort::A, 2);

        assert_eq!(output.serial(SerialPort::A), [1, 2]);
        assert_eq!(output.serial(SerialPort::B), [3]);
        assert!(!output.is_empty());
    }
}
