# garmin-connect

Unofficial async Rust client for Garmin Connect wellness data: sleep, HRV, Body Battery, stress,
resting heart rate, SpO2, respiration and Training Readiness.

Garmin has no API for personal use, so this crate logs in the way the Garmin Connect mobile
app does (SSO with optional two-factor, then a DI OAuth2 token) and reads the same endpoints.
It is not affiliated with or endorsed by Garmin, and it can break when Garmin changes things.

```rust,no_run
use garmin_connect::{FileTokenStore, GarminClient};

# async fn run() -> garmin_connect::Result<()> {
let client = GarminClient::new(FileTokenStore::new("tokens.json"))?;

// Once. The password is used for this call only; the tokens are saved.
client.login("me@example.com", "password", |_method| {
    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    Ok(code)
}).await?;

// From then on, tokens are loaded and refreshed automatically.
let date = "2026-11-07".parse().unwrap();
let sleep = client.sleep(date).await?;
let hrv = client.hrv(date).await?;
let battery = client.body_battery(date).await?;
let day = client.daily_summary(date).await?;          // stress, resting HR, SpO2, respiration
let readiness = client.training_readiness(date).await?;
# Ok(()) }
```

- **Typed summaries:** `SleepDay`, `HrvDay`, `BodyBatteryDay`, `DailySummary` and
  `TrainingReadiness` keep the useful numbers. Every field is optional, because Garmin leaves
  out whatever your watch doesn't record.
- **Raw access:** a `*_raw` method for each, and `get_json` for any other connectapi endpoint.
- **Token storage:** `FileTokenStore` writes owner-only files atomically. Implement `TokenStore`
  to keep tokens in a keychain or a database instead.
- **Errors you can act on:** `InvalidCredentials`, `RateLimited`, `Blocked`, `NotLoggedIn`, ...

The login flow follows [python-garminconnect](https://github.com/cyberjunky/python-garminconnect)
(MIT), which does the hard work of tracking Garmin's changes. For an MCP server built on this
crate, see [garmin-connector](https://github.com/peilertsen/garmin-connector).

License: MIT OR Apache-2.0.
