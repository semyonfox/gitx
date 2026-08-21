use std::io::{self, IsTerminal, Write};

use crate::error::{GitxError, Result};

/// Ask for confirmation when a preflight produced warnings.
/// Non-interactive sessions must opt in with --yes instead.
pub fn confirm_if_needed(warnings: &[String], yes: bool) -> Result<()> {
    if warnings.is_empty() || yes {
        return Ok(());
    }

    if !io::stdin().is_terminal() {
        return Err(GitxError::Message(
            "refusing to run without confirmation; pass --yes to run non-interactively".to_string(),
        ));
    }

    eprint!("Run? [y/N] ");
    io::stderr().flush().map_err(|source| GitxError::Io {
        context: "flushing confirmation prompt".to_string(),
        source,
    })?;

    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|source| GitxError::Io {
            context: "reading confirmation prompt".to_string(),
            source,
        })?;

    if matches!(line.trim(), "y" | "Y" | "yes" | "YES" | "Yes") {
        Ok(())
    } else {
        Err(GitxError::Message("run cancelled".to_string()))
    }
}
