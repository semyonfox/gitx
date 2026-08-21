use std::{
    env,
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use crate::display::{Style, colorize_stderr, terminal_safe, terminal_width};

/// Progress policy selected by the root CLI flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressMode {
    Auto,
    Always,
    Never,
}

const PROGRESS_AUTO: u8 = 0;
const PROGRESS_ALWAYS: u8 = 1;
const PROGRESS_NEVER: u8 = 2;
static PROGRESS_MODE: AtomicU8 = AtomicU8::new(PROGRESS_AUTO);
static VERBOSITY: AtomicU8 = AtomicU8::new(0);
static STDERR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

const MARCHING_MARKER: [&str; 4] = ["C---", "-C--", "--C-", "---C"];
const PROGRESS_TICK: Duration = Duration::from_millis(90);

pub(crate) fn set_progress_mode(mode: ProgressMode) {
    let value = match mode {
        ProgressMode::Auto => PROGRESS_AUTO,
        ProgressMode::Always => PROGRESS_ALWAYS,
        ProgressMode::Never => PROGRESS_NEVER,
    };
    PROGRESS_MODE.store(value, Ordering::Relaxed);
}

pub(crate) fn set_verbosity(level: u8) {
    VERBOSITY.store(level, Ordering::Relaxed);
}

/// Print a high-level operation to stderr. It is deliberately separate from
/// normal command output, so JSON remains valid on stdout.
pub(crate) fn step(message: impl AsRef<str>) {
    if VERBOSITY.load(Ordering::Relaxed) > 0 {
        write_line(&format!(
            "{} {}",
            colorize_stderr(Style::Accent, "::"),
            terminal_safe(message.as_ref())
        ));
    }
}

/// Print a more granular operation when the caller passes `-vv`.
pub(crate) fn detail(message: impl AsRef<str>) {
    if VERBOSITY.load(Ordering::Relaxed) > 1 {
        write_line(&format!(
            "{} {}",
            colorize_stderr(Style::Muted, "   ·"),
            terminal_safe(message.as_ref())
        ));
    }
}

/// Render an indeterminate terminal activity indicator for work whose byte
/// count is unavailable, such as a Git transport operation.
pub(crate) fn activity(label: impl AsRef<str>) -> Progress {
    Progress::start(label.as_ref(), None)
}

/// Render byte progress for a transfer GitX owns. Delegated package managers
/// retain control of their own download UI.
pub(crate) fn transfer(label: impl AsRef<str>, total: Option<u64>) -> Progress {
    Progress::start(label.as_ref(), total)
}

pub(crate) struct Progress {
    state: Option<Arc<ProgressState>>,
    worker: Option<thread::JoinHandle<()>>,
}

struct ProgressState {
    label: String,
    total: Option<u64>,
    transferred: AtomicU64,
    active: AtomicBool,
}

impl Progress {
    fn start(label: &str, total: Option<u64>) -> Self {
        if !progress_enabled() {
            return Self {
                state: None,
                worker: None,
            };
        }

        let state = Arc::new(ProgressState {
            label: terminal_safe(label),
            total,
            transferred: AtomicU64::new(0),
            active: AtomicBool::new(true),
        });
        render_progress(&state, 0);

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            let mut frame = 1;
            while worker_state.active.load(Ordering::Acquire) {
                thread::sleep(PROGRESS_TICK);
                if worker_state.active.load(Ordering::Acquire) {
                    render_progress(&worker_state, frame);
                    frame = (frame + 1) % MARCHING_MARKER.len();
                }
            }
        });

        Self {
            state: Some(state),
            worker: Some(worker),
        }
    }

    pub(crate) fn advance(&self, bytes: u64) {
        if let Some(state) = &self.state {
            state.transferred.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Finish a transfer with a durable final line. This is useful for a
    /// download or cache import where the completed byte count is meaningful.
    pub(crate) fn finish(mut self) {
        let Some(state) = self.stop() else {
            return;
        };

        let transferred = state.transferred.load(Ordering::Relaxed);
        let totals = match state.total {
            Some(total) => format!("{} / {}", format_bytes(transferred), format_bytes(total)),
            None => format_bytes(transferred),
        };
        write_line(&format!(
            "{} {} complete: {}",
            colorize_stderr(Style::Compatible, "::"),
            state.label,
            totals
        ));
    }

    /// Stop an activity indicator without adding a persistent completion line.
    pub(crate) fn finish_quietly(mut self) {
        let _ = self.stop();
    }

    fn stop(&mut self) -> Option<Arc<ProgressState>> {
        let state = self.state.take()?;
        state.active.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        clear_progress_line();
        Some(state)
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn progress_enabled() -> bool {
    match PROGRESS_MODE.load(Ordering::Relaxed) {
        PROGRESS_ALWAYS => true,
        PROGRESS_NEVER => false,
        _ => io::stderr().is_terminal() && !matches!(env::var("TERM").as_deref(), Ok("dumb")),
    }
}

fn render_progress(state: &ProgressState, frame: usize) {
    let transferred = state.transferred.load(Ordering::Relaxed);
    let indicator = render_indicator(state.total, transferred, frame);
    let amount = match state.total {
        Some(total) => format!(
            "{} / {} {:>3}%",
            format_bytes(transferred),
            format_bytes(total),
            percentage(transferred, total)
        ),
        None => format_bytes(transferred),
    };
    let reserved = indicator.chars().count() + amount.chars().count() + 8;
    let label = truncate(
        &state.label,
        terminal_width().saturating_sub(reserved).max(16),
    );
    write_raw(&format!(
        "\r\x1b[2K{} {} {}",
        colorize_stderr(Style::Accent, indicator),
        label,
        colorize_stderr(Style::Muted, amount)
    ));
}

fn render_indicator(total: Option<u64>, transferred: u64, frame: usize) -> String {
    let marker = MARCHING_MARKER[frame % MARCHING_MARKER.len()];
    match total.filter(|total| *total > 0) {
        Some(total) => {
            const WIDTH: usize = 18;
            let filled =
                ((transferred.min(total) as u128 * WIDTH as u128) / total as u128) as usize;
            format!(
                "[{}{}] {marker}",
                "#".repeat(filled),
                "-".repeat(WIDTH.saturating_sub(filled))
            )
        }
        None => format!("[{marker}]"),
    }
}

fn percentage(transferred: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        ((transferred.min(total) as u128 * 100) / total as u128) as u64
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width <= 3 {
        return value.chars().take(width).collect();
    }
    format!("{}...", value.chars().take(width - 3).collect::<String>())
}

fn write_line(value: &str) {
    write_raw(&format!("{value}\n"));
}

fn clear_progress_line() {
    write_raw("\r\x1b[2K");
}

fn write_raw(value: &str) {
    let lock = STDERR_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = stderr.write_all(value.as_bytes());
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, percentage, render_indicator, truncate};

    #[test]
    fn renders_a_pacman_style_marker_and_byte_bar() {
        assert_eq!(
            render_indicator(Some(100), 50, 1),
            "[#########---------] -C--"
        );
        assert_eq!(render_indicator(None, 0, 2), "[--C-]");
    }

    #[test]
    fn formats_transfer_amounts_without_false_precision() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10 MiB");
        assert_eq!(percentage(75, 100), 75);
    }

    #[test]
    fn truncates_a_progress_label_to_the_available_terminal_width() {
        assert_eq!(truncate("download package", 9), "downlo...");
        assert_eq!(truncate("download", 12), "download");
    }
}
