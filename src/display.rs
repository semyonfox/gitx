use std::{
    env,
    io::{self, IsTerminal},
    sync::atomic::{AtomicU8, Ordering},
};

/// Output colour policy selected by the root CLI flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorMode {
    Auto,
    Always,
    Never,
}

const COLOR_AUTO: u8 = 0;
const COLOR_ALWAYS: u8 = 1;
const COLOR_NEVER: u8 = 2;
static COLOR_MODE: AtomicU8 = AtomicU8::new(COLOR_AUTO);

/// The small set of terminal roles used by the interactive output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Accent,
    Heading,
    Package,
    Version,
    Source,
    Compatible,
    Incompatible,
    Unknown,
    Muted,
    Label,
    Hint,
}

pub(crate) fn set_color_mode(mode: ColorMode) {
    let value = match mode {
        ColorMode::Auto => COLOR_AUTO,
        ColorMode::Always => COLOR_ALWAYS,
        ColorMode::Never => COLOR_NEVER,
    };
    COLOR_MODE.store(value, Ordering::Relaxed);
}

pub(crate) fn colorize(style: Style, value: impl AsRef<str>) -> String {
    colorize_for_terminal(style, value, io::stdout().is_terminal())
}

/// Colour a message that is written to standard error. This matters for
/// progress and verbose output when structured data is intentionally piped
/// from standard output.
pub(crate) fn colorize_stderr(style: Style, value: impl AsRef<str>) -> String {
    colorize_for_terminal(style, value, io::stderr().is_terminal())
}

fn colorize_for_terminal(style: Style, value: impl AsRef<str>, is_terminal: bool) -> String {
    let value = value.as_ref();
    if value.is_empty() || !color_enabled(is_terminal) {
        return value.to_string();
    }

    let code = match style {
        Style::Accent => "1;35",
        Style::Heading => "1",
        Style::Package => "1;36",
        Style::Version => "36",
        Style::Source => "34",
        Style::Compatible => "32",
        Style::Incompatible => "31",
        Style::Unknown => "33",
        Style::Muted => "2",
        Style::Label => "1;37",
        Style::Hint => "2;36",
    };

    format!("\x1b[{code}m{value}\x1b[0m")
}

/// Keep line wrapping useful on ordinary terminals without trusting a huge or
/// malformed COLUMNS value from the environment.
pub(crate) fn terminal_width() -> usize {
    env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| (48..=240).contains(width))
        .unwrap_or(100)
}

pub(crate) fn visible_width(value: &str) -> usize {
    value.chars().count()
}

fn color_enabled(is_terminal: bool) -> bool {
    match COLOR_MODE.load(Ordering::Relaxed) {
        COLOR_ALWAYS => true,
        COLOR_NEVER => false,
        _ => {
            is_terminal
                && env::var_os("NO_COLOR").is_none()
                && !matches!(env::var("TERM").as_deref(), Ok("dumb"))
        }
    }
}

pub(crate) fn terminal_safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => output.push(character),
        }
    }

    output
}

/// Render one value as a copyable argument for POSIX shells. Plain package
/// names and paths stay readable; anything else is single-quoted so a search
/// provider cannot smuggle shell syntax into a suggested command.
pub(crate) fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | '/' | '@' | ':' | '+' | '=')
        })
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::{shell_quote, terminal_safe};

    #[test]
    fn escapes_terminal_control_characters() {
        assert_eq!(terminal_safe("safe\n\u{1b}[31m"), "safe\\n\\u{1b}[31m");
    }

    #[test]
    fn quotes_shell_metacharacters_without_obscuring_plain_values() {
        assert_eq!(shell_quote("npm:demo@1.2.3"), "npm:demo@1.2.3");
        assert_eq!(shell_quote("demo; rm -rf /"), "'demo; rm -rf /'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }
}
