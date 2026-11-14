//! Unofficial async client for Garmin Connect wellness data: sleep, HRV,
//! Body Battery, stress, resting heart rate, SpO2, respiration and Training Readiness.
//!
//! Garmin offers no API for personal use, so this crate logs in the way the
//! Garmin Connect mobile app does and reads the same endpoints. It can break
//! when Garmin changes its login. It is not affiliated with or endorsed by Garmin.
//!
//! ```no_run
//! use garmin_connect::{FileTokenStore, GarminClient};
//!
//! # async fn run() -> garmin_connect::Result<()> {
//! let client = GarminClient::new(FileTokenStore::new("/path/to/tokens.json"))?;
//!
//! // Once: log in with a password. Tokens are saved; the password is not.
//! client.login("me@example.com", "password", |_method| {
//!     let mut code = String::new();
//!     std::io::stdin().read_line(&mut code)?;
//!     Ok(code)
//! }).await?;
//!
//! // Afterwards: just use the client.
//! let today = chrono::Local::now().date_naive();
//! if let Some(sleep) = client.sleep(today).await? {
//!     println!("slept {:?} seconds, score {:?}", sleep.sleep_seconds, sleep.score);
//! }
//! # Ok(()) }
//! ```

mod auth;
mod client;
mod endpoints;
mod error;
pub mod models;
mod tokens;

pub use auth::{LoginOptions, MfaMethod};
pub use client::GarminClient;
pub use endpoints::Endpoints;
pub use error::{Error, Result};
pub use models::{
    BodyBatteryDay, BodyBatteryEvent, DailySummary, HrvDay, ReadinessFactor, SleepDay, TrainingReadiness,
};
pub use tokens::{FileTokenStore, MemoryTokenStore, TokenStore, Tokens};
