//! OAuth tokens and where they are kept between runs.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The credentials that let the client call the API without a password.
///
/// The refresh token grants long-lived access to the account, so treat this
/// like a password: never log it, never commit it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    /// Short-lived bearer token sent with every API request.
    pub access_token: String,
    /// Long-lived token used to get a new access token.
    pub refresh_token: Option<String>,
    /// DI OAuth client id the tokens were issued to (needed for refresh).
    pub client_id: String,
}

impl std::fmt::Debug for Tokens {
    // Hand-written so tokens never end up in logs via `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("client_id", &self.client_id)
            .finish()
    }
}

impl Tokens {
    /// Expiry of the access token as a Unix timestamp, read from the JWT.
    pub fn expires_at(&self) -> Option<i64> {
        jwt_claim(&self.access_token, "exp").and_then(|v| v.as_f64()).map(|v| v as i64)
    }

    /// True when the access token expires within `margin_secs` (or already has).
    pub fn expires_within(&self, margin_secs: i64, now: i64) -> bool {
        match self.expires_at() {
            Some(exp) => now >= exp - margin_secs,
            None => false,
        }
    }
}

/// Read one claim from a JWT payload without verifying the signature.
///
/// Only Garmin can verify its own tokens; we only read `exp` and `client_id`
/// to decide when to refresh. Tokens claiming `alg: none` are ignored.
pub(crate) fn jwt_claim(token: &str, claim: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let header = parts.next()?;
    let payload = parts.next()?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decode = |s: &str| engine.decode(s.trim_end_matches('=')).ok();
    let header: serde_json::Value = serde_json::from_slice(&decode(header)?).ok()?;
    if header.get("alg").and_then(|a| a.as_str()) == Some("none") {
        return None;
    }
    let payload: serde_json::Value = serde_json::from_slice(&decode(payload)?).ok()?;
    payload.get(claim).cloned()
}

/// Somewhere to persist tokens between runs.
///
/// Implement this to keep tokens in a keychain, a database, or anywhere else.
pub trait TokenStore: Send + Sync {
    /// Load saved tokens, or `None` if there are none yet.
    fn load(&self) -> Result<Option<Tokens>>;
    /// Save tokens, replacing any previous ones.
    fn save(&self, tokens: &Tokens) -> Result<()>;
    /// Forget saved tokens (log out).
    fn clear(&self) -> Result<()>;
}

/// Stores tokens in a JSON file readable only by the current user.
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    /// Use `path` as the token file. Its parent directory is created if needed.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Option<Tokens>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        restrict_permissions(dir, 0o700)?;

        // Write to a temporary file first and rename it into place, so a crash
        // halfway through never leaves a truncated token file behind.
        let tmp = dir.join(format!(
            ".{}.{}.tmp",
            self.path.file_name().and_then(|n| n.to_str()).unwrap_or("tokens"),
            std::process::id()
        ));
        write_private(&tmp, &serde_json::to_vec_pretty(tokens)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Create `path` with owner-only permissions from the start (no window where
/// it is world-readable) and write `bytes` to it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    restrict_permissions(path, 0o600)
}

/// Keeps tokens in memory only. Handy for tests and short-lived scripts.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: std::sync::Mutex<Option<Tokens>>,
}

impl MemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that starts with `tokens` in it.
    pub fn with_tokens(tokens: Tokens) -> Self {
        Self { tokens: std::sync::Mutex::new(Some(tokens)) }
    }
}

impl TokenStore for MemoryTokenStore {
    fn load(&self) -> Result<Option<Tokens>> {
        Ok(self.tokens.lock().expect("token mutex poisoned").clone())
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        *self.tokens.lock().expect("token mutex poisoned") = Some(tokens.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.tokens.lock().expect("token mutex poisoned") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an unsigned-looking JWT with the given payload (test helper).
    pub(crate) fn fake_jwt(payload: serde_json::Value) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = engine.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = engine.encode(payload.to_string());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn reads_expiry_and_client_id_from_jwt() {
        let jwt = fake_jwt(serde_json::json!({"exp": 1_900_000_000, "client_id": "ABC"}));
        let tokens = Tokens { access_token: jwt.clone(), refresh_token: None, client_id: "x".into() };
        assert_eq!(tokens.expires_at(), Some(1_900_000_000));
        assert_eq!(jwt_claim(&jwt, "client_id").unwrap(), "ABC");
        assert!(!tokens.expires_within(900, 1_800_000_000));
        assert!(tokens.expires_within(900, 1_899_999_500));
    }

    #[test]
    fn ignores_alg_none_tokens() {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let jwt = format!("{}.{}.", engine.encode(br#"{"alg":"none"}"#), engine.encode(br#"{"exp":1}"#));
        assert!(jwt_claim(&jwt, "exp").is_none());
    }

    #[test]
    fn debug_output_redacts_tokens() {
        let tokens = Tokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            client_id: "cid".into(),
        };
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn file_store_round_trips_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path().join("sub/tokens.json"));
        assert!(store.load().unwrap().is_none());

        let tokens =
            Tokens { access_token: "a".into(), refresh_token: Some("r".into()), client_id: "c".into() };
        store.save(&tokens).unwrap();
        assert_eq!(store.load().unwrap(), Some(tokens));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}
