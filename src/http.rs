use std::time::Duration;

use crate::error::{GitxError, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn client() -> Result<reqwest::blocking::Client> {
    build_client(REQUEST_TIMEOUT)
}

/// Search fans out to independent public providers, so it has a stricter
/// whole-request budget than a resolution or artifact operation. The caller
/// combines these bounded blocking requests on a small worker pool.
pub(crate) fn search_client() -> Result<reqwest::blocking::Client> {
    build_client(SEARCH_REQUEST_TIMEOUT)
}

fn build_client(request_timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("gitx/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .build()
        .map_err(|source| GitxError::Http {
            context: "building HTTP client".to_string(),
            source,
        })
}
