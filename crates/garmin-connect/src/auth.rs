//! Logging in to Garmin and keeping the session alive.
//!
//! The flow mirrors what the Garmin Connect iOS app does:
//!
//! 1. Load the mobile sign-in page (sets the cookies Cloudflare expects).
//! 2. Post the email and password as JSON to `/mobile/api/login`.
//! 3. If the account has two-factor auth, post the code to
//!    `/mobile/api/mfa/verifyCode`.
//! 4. Either step answers with a one-time *service ticket* (`ST-...`).
//! 5. Exchange the ticket at the DI OAuth2 endpoint for an access token
//!    and a refresh token.
//!
//! After that the password is never needed again: the access token is sent
//! with every API call and renewed with the refresh token when it expires.

use std::time::Duration;

use base64::Engine;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::endpoints::{
    DI_CLIENT_IDS, DI_GRANT_TYPE, Endpoints, IOS_LOGIN_UA, IOS_SSO_CLIENT_ID, NATIVE_API_USER_AGENT,
    NATIVE_X_GARMIN_USER_AGENT,
};
use crate::error::{Error, Result};
use crate::tokens::{Tokens, jwt_claim};

/// How the two-factor code is delivered, as reported by Garmin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaMethod {
    Email,
    Sms,
    /// An authenticator app, or a method this crate does not know yet.
    Other(String),
}

impl MfaMethod {
    fn from_garmin(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "email" => Self::Email,
            "sms" => Self::Sms,
            _ => Self::Other(s.to_string()),
        }
    }

    fn as_garmin(&self) -> &str {
        match self {
            Self::Email => "email",
            Self::Sms => "sms",
            Self::Other(s) => s,
        }
    }
}

/// Options for [`login`].
#[derive(Debug, Clone)]
pub struct LoginOptions {
    /// Pause between loading the sign-in page and submitting it. Submitting
    /// instantly looks like a bot to Cloudflare; the iOS flow uses 2-5 s.
    pub submit_delay: Duration,
}

impl Default for LoginOptions {
    fn default() -> Self {
        let jitter_ms = rand::random_range(0..3000);
        Self { submit_delay: Duration::from_millis(2000 + jitter_ms) }
    }
}

/// The JSON Garmin answers both login steps with.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SsoResponse {
    response_status: Option<ResponseStatus>,
    service_ticket_id: Option<String>,
    customer_mfa_info: Option<CustomerMfaInfo>,
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ResponseStatus {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomerMfaInfo {
    mfa_last_method_used: Option<String>,
}

/// What a login step produced.
enum Step {
    Ticket(String),
    NeedsMfa(MfaMethod),
}

/// Log in with email and password and return fresh tokens.
///
/// `mfa_code` is called only if the account uses two-factor auth. It gets
/// the delivery method and must return the code the user received.
pub async fn login<F>(
    endpoints: &Endpoints,
    email: &str,
    password: &str,
    options: &LoginOptions,
    mfa_code: F,
) -> Result<Tokens>
where
    F: FnOnce(&MfaMethod) -> std::io::Result<String>,
{
    // A cookie-keeping session: the sign-in page sets cookies the
    // credential POST must send back.
    let sso = Client::builder().cookie_store(true).timeout(Duration::from_secs(30)).build()?;

    let signin_url = format!("{}/mobile/sso/en_US/sign-in", endpoints.sso);
    let signin_query = [("clientId", IOS_SSO_CLIENT_ID), ("service", endpoints.ios_service.as_str())];

    tracing::debug!("loading mobile sign-in page");
    let page = sso
        .get(&signin_url)
        .query(&signin_query)
        .header("User-Agent", IOS_LOGIN_UA)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    check_blocked(page.status(), "sign-in page")?;

    tokio::time::sleep(options.submit_delay).await;

    // The Referer must be the sign-in page, exactly as the app's web view sends it.
    let referer = url::Url::parse_with_params(&signin_url, &signin_query)
        .map_err(|e| Error::Auth(e.to_string()))?
        .to_string();
    let api_query =
        [("clientId", IOS_SSO_CLIENT_ID), ("locale", "en-US"), ("service", endpoints.ios_service.as_str())];
    let post = |url: String, body: Value| {
        sso.post(url)
            .query(&api_query)
            .header("User-Agent", IOS_LOGIN_UA)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Origin", endpoints.sso.as_str())
            .header("Referer", referer.as_str())
            .json(&body)
            .send()
    };

    tracing::debug!("submitting credentials");
    let resp = post(
        format!("{}/mobile/api/login", endpoints.sso),
        json!({
            "username": email,
            "password": password,
            "rememberMe": true,
            "captchaToken": "",
        }),
    )
    .await?;

    let ticket = match read_sso_response(resp, "login").await? {
        Step::Ticket(ticket) => ticket,
        Step::NeedsMfa(method) => {
            let code = mfa_code(&method).map_err(|e| Error::Mfa(e.to_string()))?;
            let code = code.trim();
            if code.is_empty() {
                return Err(Error::Mfa("no code entered".into()));
            }
            tracing::debug!("submitting two-factor code");
            let resp = post(
                format!("{}/mobile/api/mfa/verifyCode", endpoints.sso),
                json!({
                    "mfaMethod": method.as_garmin(),
                    "mfaVerificationCode": code,
                    "rememberMyBrowser": true,
                    "reconsentList": [],
                    "mfaSetup": false,
                }),
            )
            .await?;
            match read_sso_response(resp, "two-factor verification").await {
                Ok(Step::Ticket(ticket)) => ticket,
                Ok(Step::NeedsMfa(_)) => return Err(Error::Mfa("Garmin asked for another code".into())),
                Err(Error::InvalidCredentials) => return Err(Error::Mfa("code was not accepted".into())),
                Err(e) => return Err(e),
            }
        }
    };

    exchange_ticket(endpoints, &ticket).await
}

/// Turn Garmin's answer to a login step into a ticket, an MFA request, or an error.
async fn read_sso_response(resp: reqwest::Response, what: &str) -> Result<Step> {
    let status = resp.status();
    check_blocked(status, what)?;
    let text = resp.text().await?;
    let body: SsoResponse = serde_json::from_str(&text)
        .map_err(|_| Error::Blocked(format!("{what} returned HTTP {status} with a non-JSON body")))?;

    let kind = body.response_status.and_then(|s| s.kind).unwrap_or_default();
    match kind.as_str() {
        "SUCCESSFUL" => body
            .service_ticket_id
            .map(Step::Ticket)
            .ok_or_else(|| Error::Auth(format!("{what} succeeded but returned no service ticket"))),
        "MFA_REQUIRED" => {
            let method =
                body.customer_mfa_info.and_then(|i| i.mfa_last_method_used).unwrap_or_else(|| "email".into());
            Ok(Step::NeedsMfa(MfaMethod::from_garmin(&method)))
        }
        "INVALID_USERNAME_PASSWORD" => Err(Error::InvalidCredentials),
        "CAPTCHA_REQUIRED" => Err(Error::Blocked(format!("{what} asked for a captcha"))),
        _ => {
            let code = body.error.as_ref().and_then(|e| e.get("status-code")).and_then(|c| c.as_str());
            if code == Some("429") {
                return Err(Error::RateLimited(format!("{what} (429 in response body)")));
            }
            // Only the status: the body can contain account details.
            Err(Error::Auth(format!(
                "{what} failed: HTTP {status}, responseStatus={}",
                if kind.is_empty() { "unknown" } else { &kind }
            )))
        }
    }
}

fn check_blocked(status: StatusCode, what: &str) -> Result<()> {
    match status.as_u16() {
        427 | 429 => Err(Error::RateLimited(format!("{what} returned {status}"))),
        403 => Err(Error::Blocked(format!("{what} returned 403 (Cloudflare challenge)"))),
        _ => Ok(()),
    }
}

/// Headers the native Garmin Connect app sends to Garmin's API hosts.
pub(crate) fn native_headers() -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut h = HeaderMap::new();
    let mut set = |k: &'static str, v: &'static str| {
        h.insert(k, HeaderValue::from_static(v));
    };
    set("User-Agent", NATIVE_API_USER_AGENT);
    set("X-Garmin-User-Agent", NATIVE_X_GARMIN_USER_AGENT);
    set("X-Garmin-Paired-App-Version", "10861");
    set("X-Garmin-Client-Platform", "Android");
    set("X-App-Ver", "10861");
    set("X-Lang", "en");
    set("X-GCExperience", "GC5");
    set("Accept-Language", "en-US,en;q=0.9");
    h
}

fn basic_auth(client_id: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:"));
    format!("Basic {encoded}")
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

fn token_client() -> Result<Client> {
    Ok(Client::builder().timeout(Duration::from_secs(30)).default_headers(native_headers()).build()?)
}

/// Exchange a one-time SSO service ticket for DI OAuth2 tokens.
async fn exchange_ticket(endpoints: &Endpoints, ticket: &str) -> Result<Tokens> {
    let http = token_client()?;
    for &client_id in DI_CLIENT_IDS {
        let resp = http
            .post(&endpoints.di_token)
            .header("Authorization", basic_auth(client_id))
            .header("Accept", "application/json,text/html;q=0.9,*/*;q=0.8")
            .header("Cache-Control", "no-cache")
            .form(&[
                ("client_id", client_id),
                ("service_ticket", ticket),
                ("grant_type", DI_GRANT_TYPE),
                ("service_url", endpoints.ios_service.as_str()),
            ])
            .send()
            .await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited("token exchange".into()));
        }
        if !resp.status().is_success() {
            tracing::debug!(client_id, status = %resp.status(), "DI client id refused the ticket");
            continue;
        }
        let body: TokenResponse = resp.json().await?;
        return Ok(tokens_from(body, client_id, None));
    }
    Err(Error::Auth("token exchange failed for every known client id".into()))
}

/// Get a new access token using the refresh token.
pub async fn refresh(endpoints: &Endpoints, tokens: &Tokens) -> Result<Tokens> {
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| Error::Auth("no refresh token saved; log in again".into()))?;
    let resp = token_client()?
        .post(&endpoints.di_token)
        .header("Authorization", basic_auth(&tokens.client_id))
        .header("Accept", "application/json")
        .header("Cache-Control", "no-cache")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", tokens.client_id.as_str()),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?;
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::RateLimited("token refresh".into()));
    }
    if !status.is_success() {
        return Err(Error::Auth(format!("token refresh failed with HTTP {status}; log in again")));
    }
    let body: TokenResponse = resp.json().await?;
    Ok(tokens_from(body, &tokens.client_id, tokens.refresh_token.clone()))
}

fn tokens_from(body: TokenResponse, fallback_client_id: &str, old_refresh: Option<String>) -> Tokens {
    // The access token names the client id it was issued to; prefer that.
    let client_id = jwt_claim(&body.access_token, "client_id")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| fallback_client_id.to_string());
    Tokens { access_token: body.access_token, refresh_token: body.refresh_token.or(old_refresh), client_id }
}
