//! Headless serial-console frontend.

use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use se_machine::serial::SerialPort;
use se_runtime::control::RuntimeState;
use se_runtime::runtime::Runtime;

use crate::terminal::HostTerminalGuard;

const ESCAPE_PREFIX: u8 = 0x1d;

/// Error returned by the headless frontend.
#[derive(Debug)]
pub struct HeadlessError {
    reason: String,
}

impl fmt::Display for HeadlessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl Error for HeadlessError {}

/// Runs a runtime through the host terminal using external Serial B.
///
/// # Errors
///
/// Returns [`HeadlessError`] when terminal setup, host I/O, runtime control,
/// guest execution, or runtime shutdown fails.
pub fn run(runtime: Runtime) -> Result<(), HeadlessError> {
    let _terminal = HostTerminalGuard::enter().map_err(headless_error)?;
    let output_error = Arc::new(Mutex::new(None));
    let handler_error = Arc::clone(&output_error);
    let mut stdout = io::stdout();
    runtime
        .set_output_handler(Box::new(move |output| {
            if handler_error
                .lock()
                .expect("output error mutex poisoned")
                .is_some()
            {
                return;
            }
            let result = stdout
                .write_all(output.serial(SerialPort::B))
                .and_then(|()| stdout.flush());
            if let Err(error) = result {
                *handler_error.lock().expect("output error mutex poisoned") = Some(error);
            }
        }))
        .map_err(headless_error)?;

    let (input_sender, input_receiver) = mpsc::channel();
    let input_thread = spawn_input_thread(input_sender).map_err(headless_error)?;
    runtime.run().map_err(headless_error)?;

    let mut normal_exit = false;
    let result = loop {
        if let Some(error) = output_error
            .lock()
            .expect("output error mutex poisoned")
            .take()
        {
            break Err(headless_error(error));
        }

        match input_receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(InputEvent::Bytes(bytes)) => {
                if let Err(error) = runtime.send_serial(SerialPort::B, &bytes) {
                    break Err(headless_error(error));
                }
            }
            Ok(InputEvent::InvalidEscape(value)) => {
                let _ = writeln!(
                    io::stderr(),
                    "sgi-emu: ignored Ctrl+] followed by 0x{value:02x}"
                );
            }
            Ok(InputEvent::Quit | InputEvent::Eof) => {
                normal_exit = true;
                break Ok(());
            }
            Ok(InputEvent::Error(error)) => break Err(headless_error(error)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                normal_exit = true;
                break Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        match runtime.status() {
            Ok(status) if status.state == RuntimeState::Paused => {
                if let Some(error) = status.last_error {
                    break Err(HeadlessError { reason: error });
                }
            }
            Ok(_) => {}
            Err(error) => break Err(headless_error(error)),
        }
    };

    runtime.shutdown().map_err(headless_error)?;
    if normal_exit {
        input_thread.join().map_err(|_| HeadlessError {
            reason: String::from("headless input thread panicked"),
        })?;
    }
    result
}

fn spawn_input_thread(sender: mpsc::Sender<InputEvent>) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(String::from("sgi-emu-stdin"))
        .spawn(move || read_input(sender))
}

fn read_input(sender: mpsc::Sender<InputEvent>) {
    read_input_from(io::stdin().lock(), sender);
}

fn read_input_from(mut input: impl Read, sender: mpsc::Sender<InputEvent>) {
    let mut processor = EscapeProcessor::default();
    let mut buffer = [0; 1024];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(InputEvent::Eof);
                return;
            }
            Ok(length) => {
                let result = processor.process(&buffer[..length]);
                if !result.bytes.is_empty() && sender.send(InputEvent::Bytes(result.bytes)).is_err()
                {
                    return;
                }
                for value in result.invalid_escapes {
                    if sender.send(InputEvent::InvalidEscape(value)).is_err() {
                        return;
                    }
                }
                if result.quit {
                    let _ = sender.send(InputEvent::Quit);
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(InputEvent::Error(error));
                return;
            }
        }
    }
}

enum InputEvent {
    Bytes(Vec<u8>),
    InvalidEscape(u8),
    Quit,
    Eof,
    Error(io::Error),
}

#[derive(Default)]
struct EscapeProcessor {
    prefix_pending: bool,
}

impl EscapeProcessor {
    fn process(&mut self, input: &[u8]) -> EscapeResult {
        let mut result = EscapeResult::default();
        for value in input {
            if self.prefix_pending {
                self.prefix_pending = false;
                match *value {
                    b'q' => {
                        result.quit = true;
                        break;
                    }
                    ESCAPE_PREFIX => result.bytes.push(ESCAPE_PREFIX),
                    _ => result.invalid_escapes.push(*value),
                }
            } else if *value == ESCAPE_PREFIX {
                self.prefix_pending = true;
            } else {
                result.bytes.push(*value);
            }
        }
        result
    }
}

#[derive(Default)]
struct EscapeResult {
    bytes: Vec<u8>,
    invalid_escapes: Vec<u8>,
    quit: bool,
}

fn headless_error(error: impl fmt::Display) -> HeadlessError {
    HeadlessError {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};
    use std::sync::mpsc;

    use super::{ESCAPE_PREFIX, EscapeProcessor, InputEvent, read_input_from};

    #[test]
    fn ordinary_and_control_bytes_pass_through_without_local_interpretation() {
        let mut processor = EscapeProcessor::default();
        let result = processor.process(b"A\x03\r");

        assert_eq!(result.bytes, b"A\x03\r");
        assert!(result.invalid_escapes.is_empty());
        assert!(!result.quit);
    }

    #[test]
    fn escape_prefix_state_survives_input_batches() {
        let mut processor = EscapeProcessor::default();
        assert!(processor.process(&[ESCAPE_PREFIX]).bytes.is_empty());

        let literal = processor.process(&[ESCAPE_PREFIX]);
        assert_eq!(literal.bytes, [ESCAPE_PREFIX]);
        assert!(!literal.quit);

        let _ = processor.process(&[ESCAPE_PREFIX]);
        let quit = processor.process(b"qignored");
        assert!(quit.quit);
        assert!(quit.bytes.is_empty());
    }

    #[test]
    fn unsupported_escape_combination_is_reported_and_discarded() {
        let mut processor = EscapeProcessor::default();
        let result = processor.process(&[ESCAPE_PREFIX, b'x', b'A']);

        assert_eq!(result.bytes, b"A");
        assert_eq!(result.invalid_escapes, [b'x']);
        assert!(!result.quit);
    }

    #[test]
    fn reader_eof_is_classified_as_a_normal_exit_event() {
        let (sender, receiver) = mpsc::channel();

        read_input_from(Cursor::new(Vec::<u8>::new()), sender);

        assert!(matches!(receiver.recv().unwrap(), InputEvent::Eof));
    }

    #[test]
    fn reader_errors_are_forwarded_without_becoming_guest_bytes() {
        struct FailingReader;

        impl Read for FailingReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("input failed"))
            }
        }

        let (sender, receiver) = mpsc::channel();
        read_input_from(FailingReader, sender);

        match receiver.recv().unwrap() {
            InputEvent::Error(error) => assert_eq!(error.to_string(), "input failed"),
            _ => panic!("the reader error was classified incorrectly"),
        }
        assert!(receiver.try_recv().is_err());
    }
}
