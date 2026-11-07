//! The authenticated API client.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};

use crate::auth::{self, LoginOptions, MfaMethod};
use crate::endpoints::Endpoints;
use crate::error::{Error, Result};
use crate::models::{BodyBatteryDay, HrvDay, SleepDay};
use crate::tokens::{TokenStore, Tokens};

/// Refresh the access token when it has less than this many seconds left.
const REFRESH_MARGIN_SECS: i64 = 15 * 60;

/// A Garmin Connect client for one account.
///
/// Cheap to share: wrap it in an `Arc` and call it from many tasks. Token
/// refreshes are serialized internally.
pub struct GarminClient {
    endpoints: Endpoints,
    http: Client,
    store: Arc<dyn TokenStore>,
    tokens: Mutex<Option<Tokens>>,
    display_name: OnceCell<String>,
}

impl std::fmt::Debug for GarminClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GarminClient").field("endpoints", &self.endpoints).finish_non_exhaustive()
    }
}

impl GarminClient {
    /// A client for a normal (non-China) Garmin account.
    pub fn new(store: impl TokenStore + 'static) -> Result<Self> {
        Self::with_endpoints(Endpoints::global(), store)
    }

    /// A client with custom endpoints (China accounts, or tests).
    pub fn with_endpoints(endpoints: Endpoints, store: impl TokenStore + 'static) -> Result<Self> {
        // No cookie store on purpose: API calls authenticate with the bearer
        // header only, so stray cookies can never leak between requests.
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .default_headers(auth::native_headers())
            .build()?;
        Ok(Self {
            endpoints,
            http,
            store: Arc::new(store),
            tokens: Mutex::new(None),
            display_name: OnceCell::new(),
        })
    }

    /// Log in with email and password, then save the tokens to the store.
    ///
    /// `mfa_code` is only called for accounts with two-factor auth.
    pub async fn login<F>(&self, email: &str, password: &str, mfa_code: F) -> Result<()>
    where
        F: FnOnce(&MfaMethod) -> std::io::Result<String>,
    {
        self.login_with_options(email, password, &LoginOptions::default(), mfa_code).await
    }

    /// Like [`login`](Self::login) with explicit options.
    pub async fn login_with_options<F>(
        &self,
        email: &str,
        password: &str,
        options: &LoginOptions,
        mfa_code: F,
    ) -> Result<()>
    where
        F: FnOnce(&MfaMethod) -> std::io::Result<String>,
    {
        let tokens = auth::login(&self.endpoints, email, password, options, mfa_code).await?;
        self.store.save(&tokens)?;
        *self.tokens.lock().await = Some(tokens);
        // Make sure the API actually accepts the new token before reporting success.
        self.display_name().await?;
        Ok(())
    }

    /// Forget the saved tokens.
    pub async fn logout(&self) -> Result<()> {
        *self.tokens.lock().await = None;
        self.store.clear()
    }

    /// Whether tokens are available (it does not check they still work).
    pub async fn has_tokens(&self) -> Result<bool> {
        Ok(self.current_tokens(false).await.is_ok())
    }

    /// Return usable tokens: load them from the store on first use and
    /// refresh them when they are about to expire (or when `force_refresh`).
    async fn current_tokens(&self, force_refresh: bool) -> Result<Tokens> {
        let mut guard = self.tokens.lock().await;
        if guard.is_none() {
            *guard = self.store.load()?;
        }
        let tokens = guard.as_ref().ok_or(Error::NotLoggedIn)?;

        let now = chrono::Utc::now().timestamp();
        if force_refresh || tokens.expires_within(REFRESH_MARGIN_SECS, now) {
            tracing::debug!("refreshing Garmin access token");
            let fresh = auth::refresh(&self.endpoints, tokens).await?;
            self.store.save(&fresh)?;
            *guard = Some(fresh);
        }
        Ok(guard.clone().expect("tokens present"))
    }

    /// Build a connectapi URL from path segments, percent-encoding each one.
    fn api_url(&self, segments: &[&str]) -> Result<url::Url> {
        let mut url = url::Url::parse(&self.endpoints.connectapi).map_err(|e| Error::Parse(e.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| Error::Parse("connectapi base URL cannot have a path".into()))?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }

    /// GET a connectapi endpoint and return its JSON (`None` for HTTP 204).
    ///
    /// This is the escape hatch for endpoints this crate has no typed method
    /// for. `segments` are the path parts, e.g. `["hrv-service", "hrv", "2026-11-07"]`.
    pub async fn get_json(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Option<Value>> {
        let url = self.api_url(segments)?;
        let mut tokens = self.current_tokens(false).await?;
        let mut retried = false;
        loop {
            let resp = self
                .http
                .get(url.clone())
                .query(query)
                .bearer_auth(&tokens.access_token)
                .header("Accept", "application/json")
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                tokens = self.current_tokens(true).await?;
                continue;
            }
            return match status {
                StatusCode::NO_CONTENT => Ok(None),
                s if s.is_success() => {
                    let text = resp.text().await?;
                    if text.trim().is_empty() { Ok(None) } else { Ok(Some(serde_json::from_str(&text)?)) }
                }
                StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited(url.path().to_string())),
                s => Err(Error::Api { status: s.as_u16(), message: error_message(resp).await }),
            };
        }
    }

    /// The account's display name, needed by some endpoints. Cached.
    pub async fn display_name(&self) -> Result<String> {
        self.display_name
            .get_or_try_init(|| async {
                let profile = self
                    .get_json(&["userprofile-service", "socialProfile"], &[])
                    .await?
                    .ok_or_else(|| Error::Parse("empty profile".into()))?;
                profile
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| Error::Parse("profile has no displayName".into()))
            })
            .await
            .cloned()
    }

    // ------------------------------------------------------------ sleep --

    /// Raw sleep response for the night ending on `date`.
    pub async fn sleep_raw(&self, date: NaiveDate) -> Result<Option<Value>> {
        let name = self.display_name().await?;
        self.get_json(
            &["wellness-service", "wellness", "dailySleepData", &name],
            &[("date", date.to_string()), ("nonSleepBufferMinutes", "60".into())],
        )
        .await
    }

    /// Sleep summary for the night ending on `date`, or `None` if none was recorded.
    pub async fn sleep(&self, date: NaiveDate) -> Result<Option<SleepDay>> {
        Ok(self.sleep_raw(date).await?.and_then(|raw| SleepDay::from_raw(date, &raw)))
    }

    // -------------------------------------------------------------- HRV --

    /// Raw HRV response for the night ending on `date`.
    pub async fn hrv_raw(&self, date: NaiveDate) -> Result<Option<Value>> {
        self.get_json(&["hrv-service", "hrv", &date.to_string()], &[]).await
    }

    /// HRV summary for the night ending on `date`, or `None` if not available.
    pub async fn hrv(&self, date: NaiveDate) -> Result<Option<HrvDay>> {
        Ok(self.hrv_raw(date).await?.and_then(|raw| HrvDay::from_raw(date, &raw)))
    }

    // ----------------------------------------------------- Body Battery --

    /// Raw Body Battery list for `start..=end`.
    pub async fn body_battery_raw(&self, start: NaiveDate, end: NaiveDate) -> Result<Option<Value>> {
        self.get_json(
            &["wellness-service", "wellness", "bodyBattery", "reports", "daily"],
            &[("startDate", start.to_string()), ("endDate", end.to_string())],
        )
        .await
    }

    /// Body Battery summary for `date`, or `None` if not available.
    pub async fn body_battery(&self, date: NaiveDate) -> Result<Option<BodyBatteryDay>> {
        Ok(self.body_battery_raw(date, date).await?.and_then(|raw| BodyBatteryDay::from_raw(date, &raw)))
    }
}

/// Pull a human-readable message out of an error response, without echoing
/// arbitrary body content (it can contain account data).
async fn error_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    let detail = resp
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned));
    detail.unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string())
}
