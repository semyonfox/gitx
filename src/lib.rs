pub mod artifact;
pub mod cache;
pub mod catalog;
mod catalog_config;
mod catalog_local;
mod catalog_providers;
pub mod cli;
mod display;
pub mod error;
pub mod git;
mod http;
pub mod inspect;
pub mod install;
pub mod manifest;
pub mod npm;
pub mod prompt;
pub mod report;
pub mod run;
pub mod scanner;
pub mod search;
pub mod selector;
pub mod source;
pub mod toolchain;

/// Render an error without allowing untrusted source, path, or subprocess text
/// to inject terminal control sequences into the CLI's stderr output.
pub fn render_error(error: &error::GitxError) -> String {
    display::terminal_safe(&error.to_string())
}
