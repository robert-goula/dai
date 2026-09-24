use std::time::Duration;

use anyhow::Result;

// devdocs.io returns an empty body without a user agent.
const USER_AGENT: &str = concat!("dai/", env!("CARGO_PKG_VERSION"));

/// Blocking HTTP client. `timeout` bounds the whole request; `None` for large
/// streamed downloads, which only get a connect timeout.
pub fn client(timeout: Option<Duration>) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .timeout(timeout)
        .build()?)
}
