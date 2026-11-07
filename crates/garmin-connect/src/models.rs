//! Typed views of Garmin's wellness responses.
//!
//! Garmin's JSON is large, undocumented, and changes without notice, so every
//! field here is optional and unknown fields are ignored. Each type keeps only
//! the numbers that are useful day to day; the raw JSON is always available
//! from the `*_raw` methods on [`crate::GarminClient`] for anything else.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------- sleep ----

/// One night of sleep, from `/wellness-service/wellness/dailySleepData`.
///
/// `date` is the day you woke up (Garmin's convention), so the sleep for the
/// night of 6 to 7 November is stored under 7 November.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SleepDay {
    pub date: NaiveDate,
    pub sleep_seconds: Option<i64>,
    pub deep_seconds: Option<i64>,
    pub light_seconds: Option<i64>,
    pub rem_seconds: Option<i64>,
    pub awake_seconds: Option<i64>,
    pub nap_seconds: Option<i64>,
    /// Sleep start/end as Unix milliseconds, UTC.
    pub start_gmt_ms: Option<i64>,
    pub end_gmt_ms: Option<i64>,
    /// Garmin's overall sleep score (0-100) and its label, e.g. `GOOD`.
    pub score: Option<i64>,
    pub score_label: Option<String>,
    /// Average overnight HRV in milliseconds.
    pub avg_overnight_hrv: Option<f64>,
    pub resting_heart_rate: Option<i64>,
    /// How much Body Battery was recharged during the night.
    pub body_battery_change: Option<i64>,
    pub avg_sleep_stress: Option<f64>,
    pub avg_spo2: Option<f64>,
    pub avg_respiration: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSleep {
    #[serde(rename = "dailySleepDTO")]
    daily_sleep_dto: Option<RawSleepDto>,
    avg_overnight_hrv: Option<f64>,
    resting_heart_rate: Option<i64>,
    body_battery_change: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSleepDto {
    calendar_date: Option<NaiveDate>,
    sleep_time_seconds: Option<i64>,
    nap_time_seconds: Option<i64>,
    deep_sleep_seconds: Option<i64>,
    light_sleep_seconds: Option<i64>,
    rem_sleep_seconds: Option<i64>,
    awake_sleep_seconds: Option<i64>,
    #[serde(rename = "sleepStartTimestampGMT")]
    sleep_start_timestamp_gmt: Option<i64>,
    #[serde(rename = "sleepEndTimestampGMT")]
    sleep_end_timestamp_gmt: Option<i64>,
    avg_sleep_stress: Option<f64>,
    #[serde(rename = "averageSpO2Value")]
    average_spo2_value: Option<f64>,
    average_respiration_value: Option<f64>,
    sleep_scores: Option<RawSleepScores>,
}

#[derive(Deserialize)]
struct RawSleepScores {
    overall: Option<RawScore>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScore {
    value: Option<i64>,
    qualifier_key: Option<String>,
}

impl SleepDay {
    /// Parse Garmin's sleep response. Returns `None` when the watch recorded
    /// no sleep for that day (Garmin still answers, just with empty fields).
    pub fn from_raw(date: NaiveDate, raw: &Value) -> Option<Self> {
        let parsed: RawSleep = serde_json::from_value(raw.clone()).ok()?;
        let dto = parsed.daily_sleep_dto?;
        dto.sleep_time_seconds?;
        let score = dto.sleep_scores.and_then(|s| s.overall);
        Some(Self {
            date: dto.calendar_date.unwrap_or(date),
            sleep_seconds: dto.sleep_time_seconds,
            deep_seconds: dto.deep_sleep_seconds,
            light_seconds: dto.light_sleep_seconds,
            rem_seconds: dto.rem_sleep_seconds,
            awake_seconds: dto.awake_sleep_seconds,
            nap_seconds: dto.nap_time_seconds,
            start_gmt_ms: dto.sleep_start_timestamp_gmt,
            end_gmt_ms: dto.sleep_end_timestamp_gmt,
            score: score.as_ref().and_then(|s| s.value),
            score_label: score.and_then(|s| s.qualifier_key),
            avg_overnight_hrv: parsed.avg_overnight_hrv,
            resting_heart_rate: parsed.resting_heart_rate,
            body_battery_change: parsed.body_battery_change,
            avg_sleep_stress: dto.avg_sleep_stress,
            avg_spo2: dto.average_spo2_value,
            avg_respiration: dto.average_respiration_value,
        })
    }
}

// ------------------------------------------------------------------ HRV ----

/// Overnight heart rate variability, from `/hrv-service/hrv/{date}`.
///
/// Only some watches record HRV; for others Garmin returns nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HrvDay {
    pub date: NaiveDate,
    /// Average HRV last night, in milliseconds.
    pub last_night_avg: Option<f64>,
    /// Highest 5-minute average last night.
    pub last_night_5min_high: Option<f64>,
    /// Rolling 7-day average.
    pub weekly_avg: Option<f64>,
    /// Garmin's status, e.g. `BALANCED`, `UNBALANCED`, `LOW`, `POOR`.
    pub status: Option<String>,
    /// Key for Garmin's feedback sentence, e.g. `HRV_BALANCED_2`.
    pub feedback: Option<String>,
    /// Your personal "balanced" range for the weekly average.
    pub baseline_low: Option<f64>,
    pub baseline_high: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHrv {
    hrv_summary: Option<RawHrvSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHrvSummary {
    calendar_date: Option<NaiveDate>,
    weekly_avg: Option<f64>,
    last_night_avg: Option<f64>,
    #[serde(rename = "lastNight5MinHigh")]
    last_night_5_min_high: Option<f64>,
    status: Option<String>,
    feedback_phrase: Option<String>,
    baseline: Option<RawHrvBaseline>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHrvBaseline {
    balanced_low: Option<f64>,
    balanced_upper: Option<f64>,
}

impl HrvDay {
    /// Parse Garmin's HRV response; `None` when there is no reading.
    pub fn from_raw(date: NaiveDate, raw: &Value) -> Option<Self> {
        let parsed: RawHrv = serde_json::from_value(raw.clone()).ok()?;
        let s = parsed.hrv_summary?;
        if s.last_night_avg.is_none() && s.weekly_avg.is_none() {
            return None;
        }
        Some(Self {
            date: s.calendar_date.unwrap_or(date),
            last_night_avg: s.last_night_avg,
            last_night_5min_high: s.last_night_5_min_high,
            weekly_avg: s.weekly_avg,
            status: s.status,
            feedback: s.feedback_phrase,
            baseline_low: s.baseline.as_ref().and_then(|b| b.balanced_low),
            baseline_high: s.baseline.and_then(|b| b.balanced_upper),
        })
    }
}

// --------------------------------------------------------- Body Battery ----

/// One day of Body Battery, from `/wellness-service/wellness/bodyBattery/reports/daily`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyBatteryDay {
    pub date: NaiveDate,
    /// Total charged and drained over the day.
    pub charged: Option<i64>,
    pub drained: Option<i64>,
    /// First reading of the day (around midnight).
    pub start_level: Option<i64>,
    pub highest: Option<i64>,
    pub lowest: Option<i64>,
    /// Most recent reading.
    pub latest_level: Option<i64>,
    /// When `latest_level` was measured, Unix milliseconds UTC.
    pub latest_at_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBodyBattery {
    date: Option<NaiveDate>,
    charged: Option<i64>,
    drained: Option<i64>,
    body_battery_values_array: Option<Vec<Vec<Value>>>,
    #[serde(rename = "bodyBatteryValueDescriptorDTOList")]
    descriptors: Option<Vec<RawDescriptor>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDescriptor {
    body_battery_value_descriptor_index: Option<usize>,
    body_battery_value_descriptor_key: Option<String>,
}

impl BodyBatteryDay {
    /// Parse one entry of Garmin's Body Battery list.
    pub fn from_raw_entry(fallback_date: NaiveDate, entry: &Value) -> Option<Self> {
        let raw: RawBodyBattery = serde_json::from_value(entry.clone()).ok()?;

        // Each sample is an array; the descriptor list says which index holds
        // what. Fall back to the usual layout [timestamp, level].
        let index_of = |key: &str, default: usize| {
            raw.descriptors
                .iter()
                .flatten()
                .find(|d| d.body_battery_value_descriptor_key.as_deref() == Some(key))
                .and_then(|d| d.body_battery_value_descriptor_index)
                .unwrap_or(default)
        };
        let ts_idx = index_of("timestamp", 0);
        let level_idx = index_of("bodyBatteryLevel", 1);

        let samples: Vec<(i64, i64)> = raw
            .body_battery_values_array
            .iter()
            .flatten()
            .filter_map(|s| Some((s.get(ts_idx)?.as_i64()?, s.get(level_idx)?.as_i64()?)))
            .collect();

        if samples.is_empty() && raw.charged.is_none() && raw.drained.is_none() {
            return None;
        }
        let latest = samples.iter().max_by_key(|(ts, _)| *ts);
        Some(Self {
            date: raw.date.unwrap_or(fallback_date),
            charged: raw.charged,
            drained: raw.drained,
            start_level: samples.iter().min_by_key(|(ts, _)| *ts).map(|(_, l)| *l),
            highest: samples.iter().map(|(_, l)| *l).max(),
            lowest: samples.iter().map(|(_, l)| *l).min(),
            latest_level: latest.map(|(_, l)| *l),
            latest_at_ms: latest.map(|(ts, _)| *ts),
        })
    }

    /// Parse Garmin's response for a single day (a list with one entry).
    pub fn from_raw(date: NaiveDate, raw: &Value) -> Option<Self> {
        let entries = raw.as_array()?;
        let entry = entries
            .iter()
            .find(|e| e.get("date").and_then(|d| d.as_str()) == Some(&date.to_string()))
            .or_else(|| entries.first())?;
        Self::from_raw_entry(date, entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn empty_sleep_is_none() {
        let raw =
            json!({"dailySleepDTO": {"id": null, "calendarDate": "2026-11-07", "sleepTimeSeconds": null}});
        assert!(SleepDay::from_raw(d("2026-11-07"), &raw).is_none());
    }

    #[test]
    fn empty_hrv_is_none() {
        assert!(HrvDay::from_raw(d("2026-11-07"), &json!({})).is_none());
        assert!(HrvDay::from_raw(d("2026-11-07"), &Value::Null).is_none());
    }

    #[test]
    fn body_battery_uses_descriptor_indexes() {
        let raw = json!([{
            "date": "2026-11-07",
            "charged": 60, "drained": 30,
            "bodyBatteryValueDescriptorDTOList": [
                {"bodyBatteryValueDescriptorIndex": 0, "bodyBatteryValueDescriptorKey": "bodyBatteryLevel"},
                {"bodyBatteryValueDescriptorIndex": 1, "bodyBatteryValueDescriptorKey": "timestamp"}
            ],
            "bodyBatteryValuesArray": [[20, 1000], [85, 2000], [70, 3000]]
        }]);
        let bb = BodyBatteryDay::from_raw(d("2026-11-07"), &raw).unwrap();
        assert_eq!(bb.start_level, Some(20));
        assert_eq!(bb.highest, Some(85));
        assert_eq!(bb.lowest, Some(20));
        assert_eq!(bb.latest_level, Some(70));
        assert_eq!(bb.latest_at_ms, Some(3000));
    }

    #[test]
    fn body_battery_empty_list_is_none() {
        assert!(BodyBatteryDay::from_raw(d("2026-11-07"), &json!([])).is_none());
    }
}
