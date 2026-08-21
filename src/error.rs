use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, GitxError>;

#[derive(Debug, Error)]
pub enum GitxError {
    #[error("invalid GitHub source `{input}`: expected github:owner/repo[@ref|#commit]")]
    InvalidGithubSource { input: String },

    #[error("invalid {host} source `{input}`: expected {host}:owner/repo[@ref|#commit]")]
    InvalidHostedGitSource { host: String, input: String },

    #[error("invalid npm source `{input}`: expected npm:<package>[@version]")]
    InvalidNpmSource { input: String },

    #[error("unsupported source `{0}`")]
    UnsupportedSource(String),

    #[error("manifest not found at {path}")]
    ManifestMissing { path: PathBuf },

    #[error("failed to read manifest at {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse manifest at {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to parse catalog configuration at {path}: {source}")]
    ParseCatalogConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error("I/O error while {context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("HTTP request failed while {context}: {source}")]
    Http {
        context: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to parse JSON while {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("npm resolution failed: {0}")]
    NpmResolution(String),

    #[error("required toolchain `{0}` was not found on PATH")]
    MissingToolchain(String),

    #[error("git failed while {context}: {stderr}")]
    Git { context: String, stderr: String },

    #[error("command `{command}` exited with status {code}")]
    ToolExit { command: String, code: i32 },

    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),

    #[error("{0}")]
    Message(String),
}
