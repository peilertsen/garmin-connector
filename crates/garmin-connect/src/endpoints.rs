//! URLs and client identity constants.
//!
//! Garmin has no public API for personal use, so this client talks to the
//! same endpoints the Garmin Connect mobile app uses. The values below mirror
//! what the maintained Python library `python-garminconnect` (0.3.x) sends;
//! if Garmin changes its login, this is the first file to look at.

/// iOS app SSO client id used for the credential login.
pub(crate) const IOS_SSO_CLIENT_ID: &str = "GCM_IOS_DARK";

/// User agent of the iOS app's login web view.
pub(crate) const IOS_LOGIN_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";

/// User agent the native app sends to the API tier.
pub(crate) const NATIVE_API_USER_AGENT: &str = "GCM-Android-5.23";
pub(crate) const NATIVE_X_GARMIN_USER_AGENT: &str = "com.garmin.android.apps.connectmobile/5.23; ; \
     Google/sdk_gphone64_arm64/google; Android/33; Dalvik/2.1.0";

/// OAuth2 grant type for exchanging an SSO service ticket for a DI token.
pub(crate) const DI_GRANT_TYPE: &str =
    "https://connectapi.garmin.com/di-oauth2-service/oauth/grant/service_ticket";

/// DI OAuth client ids, tried in order until one accepts the ticket.
pub(crate) const DI_CLIENT_IDS: &[&str] = &[
    "GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2",
    "GARMIN_CONNECT_MOBILE_ANDROID_DI_2024Q4",
    "GARMIN_CONNECT_MOBILE_ANDROID_DI",
    "GARMIN_CONNECT_MOBILE_IOS_DI",
];

/// Base URLs for one Garmin region.
///
/// Every field is overridable so tests can point the client at a local mock
/// server. Real users only ever need [`Endpoints::global`] or
/// [`Endpoints::china`].
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// SSO host, e.g. `https://sso.garmin.com`.
    pub sso: String,
    /// API host, e.g. `https://connectapi.garmin.com`.
    pub connectapi: String,
    /// DI OAuth2 token endpoint.
    pub di_token: String,
    /// Service URL the iOS login is performed for.
    pub ios_service: String,
}

impl Endpoints {
    /// Endpoints for an account on a given Garmin domain.
    pub fn for_domain(domain: &str) -> Self {
        Self {
            sso: format!("https://sso.{domain}"),
            connectapi: format!("https://connectapi.{domain}"),
            di_token: format!("https://diauth.{domain}/di-oauth2-service/oauth/token"),
            ios_service: format!("https://mobile.integration.{domain}/gcm/ios"),
        }
    }

    /// Endpoints for accounts outside China (the usual case).
    pub fn global() -> Self {
        Self::for_domain("garmin.com")
    }

    /// Endpoints for accounts registered on garmin.cn.
    pub fn china() -> Self {
        Self::for_domain("garmin.cn")
    }

    /// Point every endpoint at one base URL. Used by tests with a mock server.
    pub fn single_host(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            sso: base.to_string(),
            connectapi: base.to_string(),
            di_token: format!("{base}/di-oauth2-service/oauth/token"),
            ios_service: format!("{base}/gcm/ios"),
        }
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::global()
    }
}
