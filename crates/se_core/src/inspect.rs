//! Defines object-safe command discovery and diagnostic execution.
//!
//! [`Introspect::commands`] publishes an ordered command manifest, while
//! [`Introspect::execute`] writes command output through [`std::fmt::Write`]. A
//! command's [`InspectCommand::mutates_state`] flag declares guest-visible
//! mutation for callers; the interface does not enforce that declaration.
//! Command output is presentation text, not a stable structured-state schema.
//! Typed debugger or user-interface data belongs on a separate query surface and
//! must not be reconstructed by parsing this output.

use std::error::Error;
use std::fmt;

/// Describes one introspection command exposed by a target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectCommand {
    name: String,
    summary: String,
    mutates_state: bool,
}

impl InspectCommand {
    /// Creates a command description with its mutation declaration.
    #[must_use]
    pub fn new(name: impl Into<String>, summary: impl Into<String>, mutates_state: bool) -> Self {
        Self {
            name: name.into(),
            summary: summary.into(),
            mutates_state,
        }
    }

    /// Returns the command name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a short command summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns whether the command may change declared deterministic state.
    #[must_use]
    pub const fn mutates_state(&self) -> bool {
        self.mutates_state
    }
}

/// Reports an introspection command failure.
#[derive(Debug)]
pub enum InspectError {
    /// The requested command is not present in the command manifest.
    UnknownCommand(String),
    /// Command arguments do not satisfy the command contract.
    InvalidArguments(String),
    /// The command failed without a more specific core error.
    Failed(String),
    /// The output sink rejected formatted text.
    Output(fmt::Error),
}

impl fmt::Display for InspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command) => write!(formatter, "unknown command {command}"),
            Self::InvalidArguments(reason) => write!(formatter, "invalid arguments: {reason}"),
            Self::Failed(reason) => write!(formatter, "introspection command failed: {reason}"),
            Self::Output(error) => write!(formatter, "cannot write command output: {error}"),
        }
    }
}

impl Error for InspectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Output(error) => Some(error),
            _ => None,
        }
    }
}

impl From<fmt::Error> for InspectError {
    fn from(error: fmt::Error) -> Self {
        Self::Output(error)
    }
}

/// Exposes an object-safe text-command surface for monitors and diagnostics.
///
/// Command output has no typed compatibility contract. Structured debugger and
/// user-interface clients use a separate data model rather than parsing it.
pub trait Introspect {
    /// Returns the target's ordered command manifest.
    fn commands(&self) -> &[InspectCommand];

    /// Executes one command with tokenized arguments and a text output sink.
    ///
    /// Implementations may mutate deterministic state only when the matching
    /// [`InspectCommand`] declares that behavior. An error does not roll back text
    /// or declared state changes already produced by the command.
    ///
    /// # Errors
    ///
    /// Returns an [`InspectError`] when the command or arguments are invalid,
    /// execution fails, or the output sink rejects text.
    fn execute(
        &mut self,
        command: &str,
        arguments: &[&str],
        output: &mut dyn fmt::Write,
    ) -> Result<(), InspectError>;
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::{InspectCommand, InspectError, Introspect};

    struct Counter {
        commands: Vec<InspectCommand>,
        value: u64,
    }

    impl Introspect for Counter {
        fn commands(&self) -> &[InspectCommand] {
            &self.commands
        }

        fn execute(
            &mut self,
            command: &str,
            arguments: &[&str],
            output: &mut dyn Write,
        ) -> Result<(), InspectError> {
            if command != "show" {
                return Err(InspectError::UnknownCommand(command.to_owned()));
            }
            if !arguments.is_empty() {
                return Err(InspectError::InvalidArguments(
                    "expected no arguments".to_owned(),
                ));
            }
            write!(output, "{}", self.value)?;
            Ok(())
        }
    }

    #[test]
    fn introspection_is_object_safe() {
        let mut target: Box<dyn Introspect> = Box::new(Counter {
            commands: vec![InspectCommand::new("show", "Show the value", false)],
            value: 9,
        });
        assert_eq!(target.commands()[0].name(), "show");
        let mut output = String::new();
        target.execute("show", &[], &mut output).unwrap();
        assert_eq!(output, "9");
    }
}
