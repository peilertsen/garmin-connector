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

// -------------------------------------------------------- daily summary ----

/// The whole-day summary, from `/usersummary-service/usersummary/daily/{name}`.
///
/// One request covers stress, resting heart rate, daytime SpO2 and
/// respiration, and what drained or recharged Body Battery. For today the
/// numbers are "so far" and keep changing until the day is over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailySummary {
    pub date: NaiveDate,
    pub resting_hr: Option<i64>,
    pub resting_hr_7d_avg: Option<i64>,
    pub min_hr: Option<i64>,
    pub max_hr: Option<i64>,
    /// Average and peak stress, 0-100.
    pub avg_stress: Option<i64>,
    pub max_stress: Option<i64>,
    /// Garmin's label for the day, e.g. `CALM`, `BALANCED`, `STRESSFUL`.
    pub stress_label: Option<String>,
    pub rest_stress_seconds: Option<i64>,
    pub low_stress_seconds: Option<i64>,
    pub medium_stress_seconds: Option<i64>,
    pub high_stress_seconds: Option<i64>,
    pub activity_stress_seconds: Option<i64>,
    /// Blood oxygen in percent.
    pub avg_spo2: Option<f64>,
    pub lowest_spo2: Option<i64>,
    pub latest_spo2: Option<i64>,
    /// Breaths per minute.
    pub avg_waking_respiration: Option<f64>,
    pub lowest_respiration: Option<f64>,
    pub highest_respiration: Option<f64>,
    pub body_battery_at_wake: Option<i64>,
    /// What changed Body Battery today: sleep, activities, rest, stress.
    pub body_battery_events: Vec<BodyBatteryEvent>,
}

/// Something that charged or drained Body Battery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyBatteryEvent {
    /// E.g. `SLEEP`, `ACTIVITY`, `RECOVERY`, `NAP`, `STRESS`.
    pub event_type: Option<String>,
    pub start_gmt: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_seconds: Option<i64>,
    /// Change in Body Battery, positive when charging.
    pub impact: Option<i64>,
    /// Garmin's feedback key, e.g. `RECOVERY_BODY_BATTERY_INCREASE`.
    pub feedback: Option<String>,
    pub activity_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDailySummary {
    calendar_date: Option<NaiveDate>,
    resting_heart_rate: Option<i64>,
    last_seven_days_avg_resting_heart_rate: Option<i64>,
    min_heart_rate: Option<i64>,
    max_heart_rate: Option<i64>,
    average_stress_level: Option<i64>,
    max_stress_level: Option<i64>,
    stress_qualifier: Option<String>,
    rest_stress_duration: Option<i64>,
    low_stress_duration: Option<i64>,
    medium_stress_duration: Option<i64>,
    high_stress_duration: Option<i64>,
    activity_stress_duration: Option<i64>,
    average_spo2: Option<f64>,
    lowest_spo2: Option<i64>,
    latest_spo2: Option<i64>,
    avg_waking_respiration_value: Option<f64>,
    lowest_respiration_value: Option<f64>,
    highest_respiration_value: Option<f64>,
    body_battery_at_wake_time: Option<i64>,
    body_battery_activity_event_list: Option<Vec<RawBodyBatteryEvent>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBodyBatteryEvent {
    event_type: Option<String>,
    event_start_time_gmt: Option<String>,
    duration_in_milliseconds: Option<i64>,
    body_battery_impact: Option<i64>,
    short_feedback: Option<String>,
    activity_name: Option<Value>,
}

/// Parse Garmin's `2026-11-13T22:10:00.0` style timestamps (UTC, no zone).
pub(crate) fn parse_gmt(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Some(naive.and_utc())
}

impl DailySummary {
    /// Parse the daily summary. Returns `None` when the day has no data at all
    /// (watch not worn), which Garmin signals by leaving every value empty.
    pub fn from_raw(date: NaiveDate, raw: &Value) -> Option<Self> {
        let r: RawDailySummary = serde_json::from_value(raw.clone()).ok()?;
        if r.resting_heart_rate.is_none() && r.average_stress_level.is_none() && r.average_spo2.is_none() {
            return None;
        }
        // Garmin reports "not enough data" stress as -1/-2; treat as missing.
        let stress = |v: Option<i64>| v.filter(|x| *x >= 0);
        let events = r
            .body_battery_activity_event_list
            .unwrap_or_default()
            .into_iter()
            .map(|e| BodyBatteryEvent {
                event_type: e.event_type,
                start_gmt: e.event_start_time_gmt.as_deref().and_then(parse_gmt),
                duration_seconds: e.duration_in_milliseconds.map(|ms| ms / 1000),
                impact: e.body_battery_impact,
                feedback: e.short_feedback.filter(|f| f != "NONE"),
                activity_name: e.activity_name.and_then(|v| v.as_str().map(str::to_owned)),
            })
            .collect();
        Some(Self {
            date: r.calendar_date.unwrap_or(date),
            resting_hr: r.resting_heart_rate,
            resting_hr_7d_avg: r.last_seven_days_avg_resting_heart_rate,
            min_hr: r.min_heart_rate,
            max_hr: r.max_heart_rate,
            avg_stress: stress(r.average_stress_level),
            max_stress: stress(r.max_stress_level),
            stress_label: r.stress_qualifier.filter(|q| q != "UNKNOWN"),
            rest_stress_seconds: r.rest_stress_duration,
            low_stress_seconds: r.low_stress_duration,
            medium_stress_seconds: r.medium_stress_duration,
            high_stress_seconds: r.high_stress_duration,
            activity_stress_seconds: r.activity_stress_duration,
            avg_spo2: r.average_spo2,
            lowest_spo2: r.lowest_spo2,
            latest_spo2: r.latest_spo2,
            avg_waking_respiration: r.avg_waking_respiration_value,
            lowest_respiration: r.lowest_respiration_value,
            highest_respiration: r.highest_respiration_value,
            body_battery_at_wake: r.body_battery_at_wake_time,
            body_battery_events: events,
        })
    }
}

// ---------------------------------------------------- training readiness ----

/// Garmin's Training Readiness, from `/metrics-service/metrics/trainingreadiness/{date}`.
///
/// Garmin recalculates it during the day; this is the morning value (the one
/// shown in the Morning Report), which is what a daily plan should use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrainingReadiness {
    pub date: NaiveDate,
    /// 0-100.
    pub score: Option<i64>,
    /// E.g. `LOW`, `MODERATE`, `HIGH`, `PRIME`.
    pub level: Option<String>,
    /// Garmin's feedback keys, e.g. `LISTEN_TO_YOUR_BODY`.
    pub feedback_short: Option<String>,
    pub feedback_long: Option<String>,
    /// Minutes until fully recovered, as Garmin estimated it at the time.
    pub recovery_time_minutes: Option<i64>,
    pub acute_load: Option<i64>,
    pub hrv_weekly_avg: Option<i64>,
    pub sleep_score: Option<i64>,
    /// How each input contributes, 0-100 (higher is better), with Garmin's label.
    pub factors: Vec<ReadinessFactor>,
    /// Local time of the calculation, as Garmin reports it.
    pub calculated_at_local: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessFactor {
    pub name: String,
    pub percent: Option<i64>,
    pub feedback: Option<String>,
}

impl TrainingReadiness {
    /// Pick the morning entry (`AFTER_WAKEUP_RESET`) from Garmin's list,
    /// falling back to the earliest one. `None` when there is no entry.
    pub fn from_raw(date: NaiveDate, raw: &Value) -> Option<Self> {
        let entries: Vec<&Value> = match raw {
            Value::Array(items) => items.iter().collect(),
            Value::Object(_) => vec![raw],
            _ => return None,
        };
        let entry = entries
            .iter()
            .find(|e| e.get("inputContext").and_then(Value::as_str) == Some("AFTER_WAKEUP_RESET"))
            .or_else(|| {
                entries.iter().min_by_key(|e| e.get("timestamp").and_then(Value::as_str).unwrap_or(""))
            })?;

        let int = |k: &str| entry.get(k).and_then(Value::as_i64);
        let text = |k: &str| entry.get(k).and_then(Value::as_str).map(str::to_owned);
        let score = int("score");
        score?;

        let factors = [
            ("sleep", "sleepScore"),
            ("sleep_history", "sleepHistory"),
            ("recovery_time", "recoveryTime"),
            ("hrv", "hrv"),
            ("stress_history", "stressHistory"),
            ("training_load", "acwr"),
        ]
        .into_iter()
        .map(|(name, key)| ReadinessFactor {
            name: name.to_string(),
            percent: int(&format!("{key}FactorPercent")),
            feedback: text(&format!("{key}FactorFeedback")),
        })
        .filter(|f| f.percent.is_some() || f.feedback.is_some())
        .collect();

        Some(Self {
            date: entry
                .get("calendarDate")
                .and_then(Value::as_str)
                .and_then(|d| d.parse().ok())
                .unwrap_or(date),
            score,
            level: text("level"),
            feedback_short: text("feedbackShort"),
            feedback_long: text("feedbackLong"),
            recovery_time_minutes: int("recoveryTime"),
            acute_load: int("acuteLoad"),
            hrv_weekly_avg: int("hrvWeeklyAverage"),
            sleep_score: int("sleepScore"),
            factors,
            calculated_at_local: text("timestampLocal"),
        })
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
    fn daily_summary_ignores_negative_stress_and_empty_days() {
        let raw = json!({"calendarDate": "2026-11-07", "restingHeartRate": 50,
                         "averageStressLevel": -1, "maxStressLevel": -2, "stressQualifier": "UNKNOWN"});
        let s = DailySummary::from_raw(d("2026-11-07"), &raw).unwrap();
        assert_eq!(s.avg_stress, None);
        assert_eq!(s.max_stress, None);
        assert_eq!(s.stress_label, None);
        assert!(s.body_battery_events.is_empty());

        let empty = json!({"calendarDate": "2026-11-07", "restingHeartRate": null});
        assert!(DailySummary::from_raw(d("2026-11-07"), &empty).is_none());
    }

    #[test]
    fn readiness_falls_back_to_earliest_entry() {
        let raw = json!([
            {"timestamp": "2026-11-07T12:00:00.0", "score": 40},
            {"timestamp": "2026-11-07T06:00:00.0", "score": 70}
        ]);
        assert_eq!(TrainingReadiness::from_raw(d("2026-11-07"), &raw).unwrap().score, Some(70));
        assert!(TrainingReadiness::from_raw(d("2026-11-07"), &json!([])).is_none());
    }

    #[test]
    fn parses_garmin_gmt_timestamps() {
        assert_eq!(parse_gmt("2026-11-06T22:30:00.0").unwrap().to_rfc3339(), "2026-11-06T22:30:00+00:00");
        assert_eq!(parse_gmt("2026-11-06T22:30:00").unwrap().to_rfc3339(), "2026-11-06T22:30:00+00:00");
        assert!(parse_gmt("yesterday").is_none());
    }

    #[test]
    fn body_battery_empty_list_is_none() {
        assert!(BodyBatteryDay::from_raw(d("2026-11-07"), &json!([])).is_none());
    }
}
