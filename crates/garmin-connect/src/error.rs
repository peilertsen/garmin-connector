//! Error type for the Garmin Connect client.

/// Everything that can go wrong talking to Garmin.
///
/// The variants are split so callers can react differently: a wrong password
/// should not be retried, a rate limit should be retried *later*, and a
/// `Blocked` usually means Garmin's bot protection rejected this client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Garmin said the email or password is wrong.
    #[error("invalid Garmin username or password")]
    InvalidCredentials,

    /// The account needs a two-factor code but none could be obtained.
    #[error("two-factor authentication failed: {0}")]
    Mfa(String),

    /// Garmin answered with 429 (or its non-standard 427).
    #[error("rate limited by Garmin: {0}. Wait a while before trying again")]
    RateLimited(String),

    /// Garmin's bot protection (Cloudflare, captcha) refused the request.
    #[error("blocked by Garmin's bot protection: {0}")]
    Blocked(String),

    /// Login or token exchange failed for another reason.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// There are no tokens yet: the user has to log in first.
    #[error("not logged in to Garmin; run the login command first")]
    NotLoggedIn,

    /// The API answered with an error status.
    #[error("Garmin API error {status}: {message}")]
    Api { status: u16, message: String },

    /// A transport-level HTTP failure (DNS, TLS, timeout, ...).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// A response could not be parsed.
    #[error("unexpected response from Garmin: {0}")]
    Parse(String),

    /// Reading or writing the token store failed.
    #[error("token store error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Parse(e.to_string())
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
