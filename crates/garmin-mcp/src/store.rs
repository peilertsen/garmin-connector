//! Local history: one JSON file per metric per day.
//!
//! ```text
//! <data dir>/
//!   sleep/2026-11-07.json
//!   hrv/2026-11-07.json
//!   body_battery/2026-11-07.json
//! ```
//!
//! Each file keeps Garmin's original response next to the summary we
//! extracted, so fields we don't extract today are still on disk later.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The three metrics this connector stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Sleep,
    Hrv,
    BodyBattery,
}

impl Metric {
    fn dir_name(self) -> &'static str {
        match self {
            Metric::Sleep => "sleep",
            Metric::Hrv => "hrv",
            Metric::BodyBattery => "body_battery",
        }
    }
}

/// What is stored for one metric on one day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayRecord {
    pub date: NaiveDate,
    pub fetched_at: DateTime<Utc>,
    /// Our extracted summary, or `null` when Garmin had no data that day.
    pub summary: Value,
    /// Garmin's response exactly as received (`null` for "no content").
    pub raw: Value,
}

/// A folder of [`DayRecord`] files.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, metric: Metric, date: NaiveDate) -> PathBuf {
        self.root.join(metric.dir_name()).join(format!("{date}.json"))
    }

    /// Read a stored day, or `None` if it was never fetched.
    pub fn read(&self, metric: Metric, date: NaiveDate) -> Result<Option<DayRecord>> {
        let path = self.path(metric, date);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).with_context(|| format!("corrupt file {}", path.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write a day, replacing what was there. Atomic: write then rename.
    pub fn write(&self, metric: Metric, record: &DayRecord) -> Result<()> {
        let path = self.path(metric, record.date);
        let dir = path.parent().expect("record path has a parent");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let date: NaiveDate = "2026-11-07".parse().unwrap();
        assert!(store.read(Metric::Hrv, date).unwrap().is_none());

        let record = DayRecord {
            date,
            fetched_at: Utc::now(),
            summary: serde_json::json!({"last_night_avg": 62.0}),
            raw: serde_json::json!({"hrvSummary": {}}),
        };
        store.write(Metric::Hrv, &record).unwrap();
        assert_eq!(store.read(Metric::Hrv, date).unwrap(), Some(record));
        assert!(dir.path().join("hrv/2026-11-07.json").exists());
    }
}
