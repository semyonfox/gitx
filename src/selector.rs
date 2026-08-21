use std::{collections::HashSet, error::Error, fmt};

use crate::{
    catalog::CompatibilityStatus,
    display::{Style, colorize, terminal_safe, terminal_width, visible_width},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionCandidate {
    pub name: String,
    pub version: Option<String>,
    pub source: Option<String>,
    pub origin: Option<String>,
    pub compatibility: CompatibilityStatus,
    pub description: Option<String>,
}

impl SelectionCandidate {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            source: None,
            origin: None,
            compatibility: CompatibilityStatus::Unknown,
            description: None,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    pub fn with_compatibility(mut self, compatibility: CompatibilityStatus) -> Self {
        self.compatibility = compatibility;
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionRenderOptions {
    /// Zero-based candidate index to mark as recommended.
    pub recommended_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    EmptyInput,
    EmptyCandidates,
    InvalidToken { token: String },
    InvalidRange { token: String, reason: String },
    OutOfRange { selection: usize, max: usize },
    DuplicateSelection { selection: usize },
}

impl fmt::Display for SelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "selection cannot be empty"),
            Self::EmptyCandidates => write!(f, "cannot select from an empty candidate list"),
            Self::InvalidToken { token } => {
                write!(f, "invalid selection `{token}`: expected a number or range")
            }
            Self::InvalidRange { token, reason } => {
                write!(f, "invalid range `{token}`: {reason}")
            }
            Self::OutOfRange { selection, max } => {
                write!(f, "selection {selection} is out of range; choose 1-{max}")
            }
            Self::DuplicateSelection { selection } => {
                write!(f, "selection {selection} was provided more than once")
            }
        }
    }
}

impl Error for SelectionError {}

pub type SelectionResult<T> = std::result::Result<T, SelectionError>;

pub fn render_candidates(
    candidates: &[SelectionCandidate],
    options: SelectionRenderOptions,
) -> SelectionResult<String> {
    if let Some(index) = options.recommended_index {
        ensure_in_range(index + 1, candidates.len())?;
    }

    let number_width = candidates.len().max(1).to_string().len();
    let width = terminal_width();
    let mut output = String::new();

    for (index, candidate) in candidates.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        render_candidate(
            &mut output,
            candidate,
            index + 1,
            number_width,
            Some(index) == options.recommended_index,
            width,
        );
    }

    Ok(output)
}

fn render_candidate(
    output: &mut String,
    candidate: &SelectionCandidate,
    number: usize,
    number_width: usize,
    recommended: bool,
    width: usize,
) {
    let marker = if recommended { '>' } else { ' ' };
    let number_text = format!("{marker}{number:>number_width$}.");
    let name = terminal_safe(&candidate.name);
    let version = candidate
        .version
        .as_deref()
        .map(terminal_safe)
        .unwrap_or_else(|| "version unknown".to_string());
    let version_style = if candidate.version.is_some() {
        Style::Version
    } else {
        Style::Muted
    };
    let source = candidate.source.as_deref().map(terminal_safe);
    let status_text = compatibility_label(candidate.compatibility);
    let primary_width =
        visible_width(&number_text) + 1 + visible_width(&name) + 1 + visible_width(&version);
    let metadata_width = source
        .as_deref()
        .map(|source| visible_width(source) + 2)
        .unwrap_or_default()
        + visible_width(status_text)
        + if source.is_some() { 2 } else { 0 };
    let indent = " ".repeat(number_width + 3);

    output.push_str(&colorize(
        if recommended {
            Style::Accent
        } else {
            Style::Muted
        },
        &number_text,
    ));
    output.push(' ');
    output.push_str(&colorize(Style::Package, &name));
    output.push(' ');
    output.push_str(&colorize(version_style, &version));

    if primary_width + metadata_width + 2 <= width {
        output.push_str("  ");
        render_metadata(
            output,
            source.as_deref(),
            candidate.compatibility,
            status_text,
        );
    } else {
        output.push('\n');
        output.push_str(&indent);
        render_metadata(
            output,
            source.as_deref(),
            candidate.compatibility,
            status_text,
        );
    }

    if let Some(description) = &candidate.description {
        append_wrapped(output, &indent, "", description.trim(), width, None);
    }
    if let Some(origin) = &candidate.origin {
        append_wrapped(
            output,
            &indent,
            "origin: ",
            origin,
            width,
            Some(Style::Muted),
        );
    }
}

fn render_metadata(
    output: &mut String,
    source: Option<&str>,
    compatibility: CompatibilityStatus,
    status_text: &str,
) {
    if let Some(source) = source {
        output.push_str(&colorize(Style::Source, format!("[{source}]")));
        output.push_str("  ");
    }
    output.push_str(&colorize(compatibility_style(compatibility), status_text));
}

fn compatibility_label(status: CompatibilityStatus) -> &'static str {
    match status {
        CompatibilityStatus::Compatible => "✓ compatible",
        CompatibilityStatus::Incompatible => "× incompatible",
        CompatibilityStatus::Unknown => "? unknown",
    }
}

fn compatibility_style(status: CompatibilityStatus) -> Style {
    match status {
        CompatibilityStatus::Compatible => Style::Compatible,
        CompatibilityStatus::Incompatible => Style::Incompatible,
        CompatibilityStatus::Unknown => Style::Unknown,
    }
}

fn append_wrapped(
    output: &mut String,
    indent: &str,
    label: &str,
    value: &str,
    width: usize,
    style: Option<Style>,
) {
    let value = terminal_safe(value);
    if value.is_empty() {
        return;
    }

    let continuation = " ".repeat(visible_width(indent) + visible_width(label));
    let first_width = width
        .saturating_sub(visible_width(indent) + visible_width(label))
        .max(16);
    let continuation_width = width.saturating_sub(visible_width(&continuation)).max(16);
    let lines = wrap_text(&value, first_width.min(continuation_width));

    for (index, line) in lines.iter().enumerate() {
        output.push('\n');
        if index == 0 {
            output.push_str(indent);
            if !label.is_empty() {
                output.push_str(&colorize(Style::Muted, label));
            }
        } else {
            output.push_str(&continuation);
        }
        match style {
            Some(style) => output.push_str(&colorize(style, line)),
            None => output.push_str(line),
        }
    }
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();

    for word in value.split_whitespace() {
        if visible_width(word) > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let mut remaining = word;
            while visible_width(remaining) > width {
                let split_at = remaining
                    .char_indices()
                    .nth(width)
                    .map(|(index, _)| index)
                    .unwrap_or(remaining.len());
                lines.push(remaining[..split_at].to_string());
                remaining = &remaining[split_at..];
            }
            line.push_str(remaining);
            continue;
        }

        let next_width = visible_width(&line) + usize::from(!line.is_empty()) + visible_width(word);
        if next_width > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }

    if !line.is_empty() {
        lines.push(line);
    }

    lines
}

/// Parse one-based user selections into zero-based candidate indices.
///
/// Accepts whitespace and comma separated tokens. Each token may be a single
/// number (`1`) or an inclusive ascending range (`1-4`).
pub fn parse_selection(input: &str, candidate_count: usize) -> SelectionResult<Vec<usize>> {
    if candidate_count == 0 {
        return Err(SelectionError::EmptyCandidates);
    }

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SelectionError::EmptyInput);
    }

    let mut selected = Vec::new();
    let mut seen = HashSet::new();

    for token in trimmed.split(|char: char| char.is_whitespace() || char == ',') {
        if token.is_empty() {
            continue;
        }

        if token.contains('-') {
            let (start, end) = parse_range(token, candidate_count)?;
            for selection in start..=end {
                push_selection(selection, &mut selected, &mut seen)?;
            }
        } else {
            let selection = parse_number(token)?;
            ensure_in_range(selection, candidate_count)?;
            push_selection(selection, &mut selected, &mut seen)?;
        }
    }

    if selected.is_empty() {
        return Err(SelectionError::EmptyInput);
    }

    Ok(selected)
}

fn parse_range(token: &str, candidate_count: usize) -> SelectionResult<(usize, usize)> {
    let mut parts = token.split('-');
    let start_token = parts.next().unwrap_or_default();
    let end_token = parts.next().unwrap_or_default();

    if parts.next().is_some() || start_token.is_empty() || end_token.is_empty() {
        return Err(SelectionError::InvalidToken {
            token: token.to_string(),
        });
    }

    let start = parse_number(start_token).map_err(|_| SelectionError::InvalidToken {
        token: token.to_string(),
    })?;
    let end = parse_number(end_token).map_err(|_| SelectionError::InvalidToken {
        token: token.to_string(),
    })?;

    ensure_in_range(start, candidate_count)?;
    ensure_in_range(end, candidate_count)?;

    if start > end {
        return Err(SelectionError::InvalidRange {
            token: token.to_string(),
            reason: format!("range start {start} must be less than or equal to end {end}"),
        });
    }

    Ok((start, end))
}

fn parse_number(token: &str) -> SelectionResult<usize> {
    let selection = token
        .parse::<usize>()
        .map_err(|_| SelectionError::InvalidToken {
            token: token.to_string(),
        })?;

    if selection == 0 {
        return Err(SelectionError::InvalidToken {
            token: token.to_string(),
        });
    }

    Ok(selection)
}

fn ensure_in_range(selection: usize, candidate_count: usize) -> SelectionResult<()> {
    if candidate_count == 0 {
        return Err(SelectionError::EmptyCandidates);
    }

    if selection > candidate_count {
        return Err(SelectionError::OutOfRange {
            selection,
            max: candidate_count,
        });
    }

    Ok(())
}

fn push_selection(
    one_based_selection: usize,
    selected: &mut Vec<usize>,
    seen: &mut HashSet<usize>,
) -> SelectionResult<()> {
    if !seen.insert(one_based_selection) {
        return Err(SelectionError::DuplicateSelection {
            selection: one_based_selection,
        });
    }

    selected.push(one_based_selection - 1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<SelectionCandidate> {
        vec![
            SelectionCandidate::new("prettier")
                .with_version("3.6.2")
                .with_source("npm")
                .with_origin("npm registry")
                .with_description("Opinionated code formatter"),
            SelectionCandidate::new("pretty-quick")
                .with_version("4.2.2")
                .with_source("npm")
                .with_origin("npm registry")
                .with_description("Runs prettier on changed files"),
            SelectionCandidate::new("prettier-plugin-tailwindcss")
                .with_version("0.6.13")
                .with_source("npm")
                .with_origin("npm registry"),
        ]
    }

    #[test]
    fn renders_compact_numbered_candidates() {
        let output = render_candidates(&candidates(), SelectionRenderOptions::default()).unwrap();

        assert!(output.contains(" 1. prettier 3.6.2  [npm]  ? unknown"));
        assert!(output.contains("    Opinionated code formatter"));
        assert!(output.contains("    origin: npm registry"));
    }

    #[test]
    fn escapes_control_characters_in_registry_candidates() {
        let output = render_candidates(
            &[SelectionCandidate::new("safe\u{1b}[31m").with_description("line one\nline two")],
            SelectionRenderOptions::default(),
        )
        .unwrap();

        assert_eq!(
            output,
            " 1. safe\\u{1b}[31m version unknown  ? unknown\n    line one\\nline two"
        );
    }

    #[test]
    fn marks_recommended_candidate() {
        let output = render_candidates(
            &candidates(),
            SelectionRenderOptions {
                recommended_index: Some(1),
            },
        )
        .unwrap();

        assert!(output.lines().any(|line| line.starts_with(">2.")));
    }

    #[test]
    fn rejects_out_of_range_recommended_candidate() {
        let error = render_candidates(
            &candidates(),
            SelectionRenderOptions {
                recommended_index: Some(3),
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "selection 4 is out of range; choose 1-3");
    }

    #[test]
    fn parses_single_selection() {
        assert_eq!(parse_selection("1", 4).unwrap(), vec![0]);
    }

    #[test]
    fn parses_multiple_selections() {
        assert_eq!(parse_selection("1 3", 4).unwrap(), vec![0, 2]);
    }

    #[test]
    fn parses_comma_separated_selections() {
        assert_eq!(parse_selection("1,3", 4).unwrap(), vec![0, 2]);
    }

    #[test]
    fn parses_inclusive_ranges() {
        assert_eq!(parse_selection("1-4", 5).unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn parses_mixed_tokens() {
        assert_eq!(parse_selection("1 3-4", 5).unwrap(), vec![0, 2, 3]);
    }

    #[test]
    fn rejects_empty_selection() {
        assert_eq!(
            parse_selection("   ", 3).unwrap_err(),
            SelectionError::EmptyInput
        );
    }

    #[test]
    fn rejects_selection_when_no_candidates_exist() {
        assert_eq!(
            parse_selection("1", 0).unwrap_err(),
            SelectionError::EmptyCandidates
        );
    }

    #[test]
    fn rejects_invalid_token() {
        assert_eq!(
            parse_selection("wat", 3).unwrap_err(),
            SelectionError::InvalidToken {
                token: "wat".to_string()
            }
        );
    }

    #[test]
    fn rejects_zero() {
        assert_eq!(
            parse_selection("0", 3).unwrap_err(),
            SelectionError::InvalidToken {
                token: "0".to_string()
            }
        );
    }

    #[test]
    fn rejects_out_of_range_selection() {
        assert_eq!(
            parse_selection("4", 3).unwrap_err(),
            SelectionError::OutOfRange {
                selection: 4,
                max: 3
            }
        );
    }

    #[test]
    fn rejects_descending_range() {
        assert_eq!(
            parse_selection("3-1", 3).unwrap_err(),
            SelectionError::InvalidRange {
                token: "3-1".to_string(),
                reason: "range start 3 must be less than or equal to end 1".to_string()
            }
        );
    }

    #[test]
    fn rejects_malformed_range() {
        assert_eq!(
            parse_selection("1-2-3", 5).unwrap_err(),
            SelectionError::InvalidToken {
                token: "1-2-3".to_string()
            }
        );
    }

    #[test]
    fn rejects_duplicate_selection() {
        assert_eq!(
            parse_selection("1 1", 3).unwrap_err(),
            SelectionError::DuplicateSelection { selection: 1 }
        );
    }
}
