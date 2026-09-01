//! Host terminal-mode ownership for the headless frontend.

use std::io;

/// Restores the host terminal state when dropped.
pub struct HostTerminalGuard {
    #[cfg(unix)]
    original: rustix::termios::Termios,
    #[cfg(windows)]
    input_handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    input_mode: u32,
    #[cfg(windows)]
    output_handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    output_mode: u32,
}

impl HostTerminalGuard {
    /// Enters host raw mode while preserving the exact original settings.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when standard input or output is not an available
    /// terminal or its mode cannot be changed.
    pub fn enter() -> io::Result<Self> {
        enter_terminal()
    }
}

#[cfg(unix)]
fn enter_terminal() -> io::Result<HostTerminalGuard> {
    use rustix::stdio::stdin;
    use rustix::termios::{OptionalActions, tcgetattr, tcsetattr};

    let original = tcgetattr(stdin()).map_err(io::Error::from)?;
    let mut raw = original.clone();
    raw.make_raw();
    tcsetattr(stdin(), OptionalActions::Now, &raw).map_err(io::Error::from)?;
    Ok(HostTerminalGuard { original })
}

#[cfg(windows)]
fn enter_terminal() -> io::Result<HostTerminalGuard> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
        GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    // SAFETY: the returned standard handles are checked before use and remain
    // process-owned for the guard's lifetime.
    let (input_handle, output_handle) = unsafe {
        (
            GetStdHandle(STD_INPUT_HANDLE),
            GetStdHandle(STD_OUTPUT_HANDLE),
        )
    };
    if input_handle == INVALID_HANDLE_VALUE
        || input_handle.is_null()
        || output_handle == INVALID_HANDLE_VALUE
        || output_handle.is_null()
    {
        return Err(io::Error::last_os_error());
    }

    let mut input_mode = 0;
    let mut output_mode = 0;
    // SAFETY: both handles were validated and the output pointers refer to
    // initialized local storage.
    if unsafe { GetConsoleMode(input_handle, &mut input_mode) } == 0
        || unsafe { GetConsoleMode(output_handle, &mut output_mode) } == 0
    {
        return Err(io::Error::last_os_error());
    }

    let raw_input = input_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT)
        | ENABLE_VIRTUAL_TERMINAL_INPUT;
    // SAFETY: the validated console handle remains live and the mode uses only
    // documented console input flags.
    if unsafe { SetConsoleMode(input_handle, raw_input) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the validated console handle remains live and the mode preserves
    // every original flag while enabling virtual-terminal output processing.
    if unsafe {
        SetConsoleMode(
            output_handle,
            output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        )
    } == 0
    {
        // SAFETY: restoring the exact saved mode is the cleanup action for a
        // partial setup.
        unsafe {
            SetConsoleMode(input_handle, input_mode);
        }
        return Err(io::Error::last_os_error());
    }

    Ok(HostTerminalGuard {
        input_handle,
        input_mode,
        output_handle,
        output_mode,
    })
}

#[cfg(unix)]
impl Drop for HostTerminalGuard {
    fn drop(&mut self) {
        use rustix::stdio::stdin;
        use rustix::termios::{OptionalActions, tcsetattr};

        let _ = tcsetattr(stdin(), OptionalActions::Now, &self.original);
    }
}

#[cfg(windows)]
impl Drop for HostTerminalGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleMode;

        // SAFETY: the standard console handles remain process-owned, and the
        // saved values are the exact modes obtained during construction.
        unsafe {
            SetConsoleMode(self.input_handle, self.input_mode);
            SetConsoleMode(self.output_handle, self.output_mode);
        }
    }
}
