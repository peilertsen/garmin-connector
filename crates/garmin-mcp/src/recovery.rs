//! Combines the Garmin client with the local store: fetches what is missing
//! or stale, and turns stored days into the summaries Claude sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use futures::stream::{self, StreamExt};
use garmin_connect::{
    BodyBatteryDay, BodyBatteryEvent, DailySummary, GarminClient, HrvDay, SleepDay, TrainingReadiness,
};
use serde::Serialize;
use serde_json::Value;

use crate::store::{DayRecord, Metric, Store};

/// Days older than this are treated as final and never re-fetched.
const SETTLED_AFTER_DAYS: i64 = 2;
/// Recent days are re-fetched when the stored copy is older than this.
const RECENT_MAX_AGE_MINUTES: i64 = 30;
/// Days used for the personal baseline.
pub const BASELINE_DAYS: i64 = 28;
/// Longest range one history request may cover.
pub const MAX_RANGE_DAYS: i64 = 92;
/// Parallel requests when backfilling history.
const CONCURRENCY: usize = 4;
/// Metrics fetched one request per day.
const PER_DAY_METRICS: [Metric; 4] =
    [Metric::Sleep, Metric::Hrv, Metric::DailySummary, Metric::TrainingReadiness];

/// Where "now" and "today" come from. Swappable so tests are deterministic.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn today(&self) -> NaiveDate {
        self.now().with_timezone(&Local).date_naive()
    }
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub struct Recovery {
    client: Arc<GarminClient>,
    store: Store,
    clock: Box<dyn Clock>,
}

impl Recovery {
    pub fn new(client: Arc<GarminClient>, store: Store) -> Self {
        Self { client, store, clock: Box::new(SystemClock) }
    }

    #[cfg(test)]
    pub fn with_clock(client: Arc<GarminClient>, store: Store, clock: Box<dyn Clock>) -> Self {
        Self { client, store, clock }
    }

    pub fn today(&self) -> NaiveDate {
        self.clock.today()
    }

    fn is_fresh(&self, record: &DayRecord) -> bool {
        let today = self.clock.today();
        if record.date < today - Duration::days(SETTLED_AFTER_DAYS) {
            // Fetched after the day had settled: nothing will change any more.
            return record.fetched_at.with_timezone(&Local).date_naive()
                >= record.date + Duration::days(SETTLED_AFTER_DAYS);
        }
        self.clock.now() - record.fetched_at < Duration::minutes(RECENT_MAX_AGE_MINUTES)
    }

    fn record(&self, date: NaiveDate, summary: Value, raw: Option<Value>) -> DayRecord {
        DayRecord { date, fetched_at: self.clock.now(), summary, raw: raw.unwrap_or(Value::Null) }
    }

    async fn fetch_one(&self, metric: Metric, date: NaiveDate) -> Result<DayRecord> {
        let c = &self.client;
        let (raw, summary) = match metric {
            Metric::Sleep => {
                let raw = c.sleep_raw(date).await?;
                let s = raw.as_ref().and_then(|r| SleepDay::from_raw(date, r));
                (raw, serde_json::to_value(s)?)
            }
            Metric::Hrv => {
                let raw = c.hrv_raw(date).await?;
                let s = raw.as_ref().and_then(|r| HrvDay::from_raw(date, r));
                (raw, serde_json::to_value(s)?)
            }
            Metric::BodyBattery => {
                let raw = c.body_battery_raw(date, date).await?;
                let s = raw.as_ref().and_then(|r| BodyBatteryDay::from_raw(date, r));
                (raw, serde_json::to_value(s)?)
            }
            Metric::DailySummary => {
                let raw = c.daily_summary_raw(date).await?;
                let s = raw.as_ref().and_then(|r| DailySummary::from_raw(date, r));
                (raw, serde_json::to_value(s)?)
            }
            Metric::TrainingReadiness => {
                let raw = c.training_readiness_raw(date).await?;
                let s = raw.as_ref().and_then(|r| TrainingReadiness::from_raw(date, r));
                (raw, serde_json::to_value(s)?)
            }
        };
        let record = self.record(date, summary, raw);
        self.store.write(metric, &record)?;
        Ok(record)
    }

    /// Body Battery has a range endpoint, so fetch many days in one request.
    async fn fetch_body_battery_range(&self, dates: &[NaiveDate]) -> Result<()> {
        let (Some(&start), Some(&end)) = (dates.iter().min(), dates.iter().max()) else {
            return Ok(());
        };
        let raw = self.client.body_battery_raw(start, end).await?;
        let entries: Vec<Value> = raw.and_then(|r| r.as_array().cloned()).unwrap_or_default();
        for &date in dates {
            let entry = entries
                .iter()
                .find(|e| e.get("date").and_then(|d| d.as_str()) == Some(&date.to_string()))
                .cloned();
            let summary = entry.as_ref().and_then(|e| BodyBatteryDay::from_raw_entry(date, e));
            // Store the day's entry in the same shape as a single-day response.
            let raw_day = entry.map(|e| Value::Array(vec![e]));
            self.store
                .write(Metric::BodyBattery, &self.record(date, serde_json::to_value(summary)?, raw_day))?;
        }
        Ok(())
    }

    /// Make sure every metric for `start..=end` is stored and fresh.
    ///
    /// Returns warnings for days that could not be fetched. Stale copies are
    /// kept and used when a fetch fails.
    pub async fn ensure(&self, start: NaiveDate, end: NaiveDate) -> Result<Vec<String>> {
        let today = self.clock.today();
        let end = end.min(today);
        let mut warnings = Vec::new();
        if start > end {
            return Ok(warnings);
        }
        let dates: Vec<NaiveDate> = start.iter_days().take_while(|d| *d <= end).collect();

        let needs = |metric: Metric| -> Result<Vec<NaiveDate>> {
            let mut out = Vec::new();
            for &d in &dates {
                match self.store.read(metric, d)? {
                    Some(r) if self.is_fresh(&r) => {}
                    _ => out.push(d),
                }
            }
            Ok(out)
        };

        let bb = needs(Metric::BodyBattery)?;
        if let Err(e) = self.fetch_body_battery_range(&bb).await {
            warnings.push(format!("Body Battery: {e}"));
        }

        let mut jobs = Vec::new();
        for metric in PER_DAY_METRICS {
            jobs.extend(needs(metric)?.into_iter().map(|d| (metric, d)));
        }
        let results: Vec<_> = stream::iter(jobs)
            .map(|(metric, date)| async move { (metric, date, self.fetch_one(metric, date).await) })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;
        for (metric, date, result) in results {
            if let Err(e) = result {
                warnings.push(format!("{metric:?} {date}: {e}"));
            }
        }
        // If login is broken every request fails the same way; say it once.
        if warnings.iter().any(|w| w.contains("not logged in")) {
            bail!(garmin_connect::Error::NotLoggedIn);
        }
        warnings.sort();
        warnings.dedup();
        Ok(warnings)
    }

    fn stored_raw(&self, metric: Metric, date: NaiveDate) -> Result<Option<Value>> {
        Ok(self.store.read(metric, date)?.map(|r| r.raw).filter(|r| !r.is_null()))
    }

    /// The stored summaries for one day (no network access). Body Battery
    /// events are only included when `with_events` is set, to keep long
    /// histories compact.
    pub fn stored_day(&self, date: NaiveDate, with_events: bool) -> Result<DayView> {
        let sleep = self.stored_raw(Metric::Sleep, date)?.and_then(|r| SleepDay::from_raw(date, &r));
        let hrv = self.stored_raw(Metric::Hrv, date)?.and_then(|r| HrvDay::from_raw(date, &r));
        let bb = self.stored_raw(Metric::BodyBattery, date)?.and_then(|r| BodyBatteryDay::from_raw(date, &r));
        let daily =
            self.stored_raw(Metric::DailySummary, date)?.and_then(|r| DailySummary::from_raw(date, &r));
        let readiness = self
            .stored_raw(Metric::TrainingReadiness, date)?
            .and_then(|r| TrainingReadiness::from_raw(date, &r));
        Ok(DayView::new(
            date,
            Sources {
                sleep: sleep.as_ref(),
                hrv: hrv.as_ref(),
                body_battery: bb.as_ref(),
                daily: daily.as_ref(),
                readiness: readiness.as_ref(),
            },
            with_events,
        ))
    }

    /// Everything the morning briefing needs for `date`.
    pub async fn snapshot(&self, date: NaiveDate) -> Result<Snapshot> {
        let baseline_start = date - Duration::days(BASELINE_DAYS);
        let mut notes = self.ensure(baseline_start, date).await?;

        let day = self.stored_day(date, true)?;
        let history: Vec<DayView> = baseline_start
            .iter_days()
            .take_while(|d| *d < date)
            .map(|d| self.stored_day(d, false))
            .collect::<Result<_>>()?;
        let stress_yesterday = history.last().and_then(|d| d.stress.clone());
        let baseline = Baseline::from_days(&history);
        let vs_baseline = Comparison::new(&day, stress_yesterday.as_ref(), &baseline);

        if day.sleep.is_none() {
            notes.push("No sleep recorded for last night (watch not worn or not synced yet).".into());
        }
        if day.hrv.is_none() {
            notes.push("No HRV reading for last night. Some watches don't record HRV.".into());
        }
        if day.training_readiness.is_none() {
            notes
                .push("No Training Readiness yet (calculated after waking; not all watches have it).".into());
        }
        if date == self.clock.today() && day.stress.is_some() {
            notes.push("`stress` covers today so far; `stress_yesterday` is the last full day.".into());
        }
        Ok(Snapshot { day, stress_yesterday, baseline_28d: baseline, vs_baseline, notes })
    }

    /// One summary per day for `start..=end`.
    pub async fn history(&self, start: NaiveDate, end: NaiveDate) -> Result<History> {
        if end < start {
            bail!("end_date is before start_date");
        }
        if (end - start).num_days() >= MAX_RANGE_DAYS {
            bail!("range too long: at most {MAX_RANGE_DAYS} days per request");
        }
        let notes = self.ensure(start, end).await?;
        let days = start
            .iter_days()
            .take_while(|d| *d <= end)
            .map(|d| self.stored_day(d, false))
            .collect::<Result<_>>()?;
        Ok(History { days, notes })
    }

    /// Garmin's original response for one metric and day.
    pub async fn raw(&self, metric: Metric, date: NaiveDate) -> Result<Value> {
        let notes = self.ensure(date, date).await?;
        match self.store.read(metric, date)? {
            Some(r) => Ok(r.raw),
            None => bail!("no data stored for {metric:?} on {date}: {}", notes.join("; ")),
        }
    }
}

// ------------------------------------------------------------ views ----

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn minutes(secs: Option<i64>) -> Option<i64> {
    secs.map(|s| (s as f64 / 60.0).round() as i64)
}

fn local_time(ms: Option<i64>) -> Option<String> {
    let dt = Local.timestamp_millis_opt(ms?).single()?;
    Some(dt.format("%H:%M").to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SleepView {
    pub hours: Option<f64>,
    pub score: Option<i64>,
    pub score_label: Option<String>,
    pub bedtime: Option<String>,
    pub wake_time: Option<String>,
    pub deep_min: Option<i64>,
    pub light_min: Option<i64>,
    pub rem_min: Option<i64>,
    pub awake_min: Option<i64>,
    pub avg_overnight_hrv_ms: Option<f64>,
    pub body_battery_recharged: Option<i64>,
    pub avg_sleep_stress: Option<f64>,
}

impl From<&SleepDay> for SleepView {
    fn from(s: &SleepDay) -> Self {
        Self {
            hours: s.sleep_seconds.map(|v| round1(v as f64 / 3600.0)),
            score: s.score,
            score_label: s.score_label.clone(),
            bedtime: local_time(s.start_gmt_ms),
            wake_time: local_time(s.end_gmt_ms),
            deep_min: minutes(s.deep_seconds),
            light_min: minutes(s.light_seconds),
            rem_min: minutes(s.rem_seconds),
            awake_min: minutes(s.awake_seconds),
            avg_overnight_hrv_ms: s.avg_overnight_hrv,
            body_battery_recharged: s.body_battery_change,
            avg_sleep_stress: s.avg_sleep_stress,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HrvView {
    pub last_night_avg_ms: Option<f64>,
    pub last_night_5min_high_ms: Option<f64>,
    pub weekly_avg_ms: Option<f64>,
    pub status: Option<String>,
    pub balanced_range_ms: Option<[f64; 2]>,
    pub feedback: Option<String>,
}

impl From<&HrvDay> for HrvView {
    fn from(h: &HrvDay) -> Self {
        Self {
            last_night_avg_ms: h.last_night_avg,
            last_night_5min_high_ms: h.last_night_5min_high,
            weekly_avg_ms: h.weekly_avg,
            status: h.status.clone(),
            balanced_range_ms: h.baseline_low.zip(h.baseline_high).map(|(a, b)| [a, b]),
            feedback: h.feedback.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FactorView {
    /// 0-100, higher is better for readiness.
    pub percent: Option<i64>,
    pub rating: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReadinessView {
    pub score: Option<i64>,
    pub level: Option<String>,
    pub feedback: Option<String>,
    pub recovery_time_hours: Option<f64>,
    pub acute_load: Option<i64>,
    pub factors: BTreeMap<String, FactorView>,
    pub calculated_at: Option<String>,
}

impl From<&TrainingReadiness> for ReadinessView {
    fn from(t: &TrainingReadiness) -> Self {
        Self {
            score: t.score,
            level: t.level.clone(),
            feedback: t.feedback_short.clone(),
            recovery_time_hours: t.recovery_time_minutes.map(|m| round1(m as f64 / 60.0)),
            acute_load: t.acute_load,
            factors: t
                .factors
                .iter()
                .map(|f| (f.name.clone(), FactorView { percent: f.percent, rating: f.feedback.clone() }))
                .collect(),
            // "2026-11-07T07:35:00.0" -> "07:35"
            calculated_at: t.calculated_at_local.as_deref().and_then(|s| s.get(11..16)).map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EventView {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub start: Option<String>,
    pub duration_min: Option<i64>,
    /// Change in Body Battery: positive charged, negative drained.
    pub impact: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

impl From<&BodyBatteryEvent> for EventView {
    fn from(e: &BodyBatteryEvent) -> Self {
        Self {
            kind: e.event_type.clone(),
            start: e.start_gmt.map(|t| t.with_timezone(&Local).format("%H:%M").to_string()),
            duration_min: minutes(e.duration_seconds),
            impact: e.impact,
            activity: e.activity_name.clone(),
            feedback: e.feedback.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BodyBatteryView {
    pub latest: Option<i64>,
    pub latest_at: Option<String>,
    pub at_wake: Option<i64>,
    pub highest: Option<i64>,
    pub lowest: Option<i64>,
    pub charged: Option<i64>,
    pub drained: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<EventView>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StressView {
    /// 0-100: 0-25 rest, 26-50 low, 51-75 medium, 76-100 high.
    pub avg: Option<i64>,
    pub max: Option<i64>,
    pub label: Option<String>,
    pub rest_min: Option<i64>,
    pub low_min: Option<i64>,
    pub medium_min: Option<i64>,
    pub high_min: Option<i64>,
    pub activity_min: Option<i64>,
}

impl StressView {
    fn from_daily(d: &DailySummary) -> Option<Self> {
        d.avg_stress?;
        Some(Self {
            avg: d.avg_stress,
            max: d.max_stress,
            label: d.stress_label.clone(),
            rest_min: minutes(d.rest_stress_seconds),
            low_min: minutes(d.low_stress_seconds),
            medium_min: minutes(d.medium_stress_seconds),
            high_min: minutes(d.high_stress_seconds),
            activity_min: minutes(d.activity_stress_seconds),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RestingHrView {
    pub bpm: Option<i64>,
    pub avg_7d: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Spo2View {
    /// Percent.
    pub sleep_avg: Option<f64>,
    pub day_avg: Option<f64>,
    pub lowest: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RespirationView {
    /// Breaths per minute.
    pub sleep_avg: Option<f64>,
    pub waking_avg: Option<f64>,
    pub lowest: Option<f64>,
    pub highest: Option<f64>,
}

/// What one day's stored files provide (any of them may be missing).
#[derive(Default, Clone, Copy)]
pub struct Sources<'a> {
    pub sleep: Option<&'a SleepDay>,
    pub hrv: Option<&'a HrvDay>,
    pub body_battery: Option<&'a BodyBatteryDay>,
    pub daily: Option<&'a DailySummary>,
    pub readiness: Option<&'a TrainingReadiness>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DayView {
    pub date: NaiveDate,
    pub sleep: Option<SleepView>,
    pub hrv: Option<HrvView>,
    pub training_readiness: Option<ReadinessView>,
    pub body_battery: Option<BodyBatteryView>,
    pub stress: Option<StressView>,
    pub resting_hr: Option<RestingHrView>,
    pub spo2: Option<Spo2View>,
    pub respiration: Option<RespirationView>,
}

/// `Some(x)` unless every field is empty.
fn non_empty<T>(value: T, any: bool) -> Option<T> {
    any.then_some(value)
}

impl DayView {
    pub fn new(date: NaiveDate, src: Sources<'_>, with_events: bool) -> Self {
        let Sources { sleep, hrv, body_battery: bb, daily, readiness } = src;

        let body_battery =
            (bb.is_some() || daily.and_then(|d| d.body_battery_at_wake).is_some()).then(|| BodyBatteryView {
                latest: bb.and_then(|b| b.latest_level),
                latest_at: bb.and_then(|b| local_time(b.latest_at_ms)),
                at_wake: daily.and_then(|d| d.body_battery_at_wake),
                highest: bb.and_then(|b| b.highest),
                lowest: bb.and_then(|b| b.lowest),
                charged: bb.and_then(|b| b.charged),
                drained: bb.and_then(|b| b.drained),
                events: (with_events)
                    .then(|| daily.map(|d| d.body_battery_events.iter().map(Into::into).collect()))
                    .flatten(),
            });

        let rhr_bpm = daily.and_then(|d| d.resting_hr).or_else(|| sleep.and_then(|s| s.resting_heart_rate));
        let rhr_7d = daily.and_then(|d| d.resting_hr_7d_avg);
        let resting_hr = non_empty(RestingHrView { bpm: rhr_bpm, avg_7d: rhr_7d }, rhr_bpm.is_some());

        let spo2 = Spo2View {
            sleep_avg: sleep.and_then(|s| s.avg_spo2),
            day_avg: daily.and_then(|d| d.avg_spo2),
            lowest: daily.and_then(|d| d.lowest_spo2),
        };
        let any_spo2 = spo2.sleep_avg.is_some() || spo2.day_avg.is_some() || spo2.lowest.is_some();

        let resp = RespirationView {
            sleep_avg: sleep.and_then(|s| s.avg_respiration),
            waking_avg: daily.and_then(|d| d.avg_waking_respiration),
            lowest: daily.and_then(|d| d.lowest_respiration),
            highest: daily.and_then(|d| d.highest_respiration),
        };
        let any_resp = resp.sleep_avg.is_some() || resp.waking_avg.is_some();

        Self {
            date,
            sleep: sleep.map(Into::into),
            hrv: hrv.map(Into::into),
            training_readiness: readiness.map(Into::into),
            body_battery,
            stress: daily.and_then(StressView::from_daily),
            resting_hr,
            spo2: non_empty(spo2, any_spo2),
            respiration: non_empty(resp, any_resp),
        }
    }

    fn sleep_hours(&self) -> Option<f64> {
        self.sleep.as_ref()?.hours
    }
    fn sleep_score(&self) -> Option<f64> {
        self.sleep.as_ref()?.score.map(|v| v as f64)
    }
    fn hrv(&self) -> Option<f64> {
        // Prefer the HRV service; fall back to the average in the sleep data.
        self.hrv
            .as_ref()
            .and_then(|h| h.last_night_avg_ms)
            .or_else(|| self.sleep.as_ref().and_then(|s| s.avg_overnight_hrv_ms))
    }
    fn resting_hr(&self) -> Option<f64> {
        self.resting_hr.as_ref()?.bpm.map(|v| v as f64)
    }
    fn bb_peak(&self) -> Option<f64> {
        self.body_battery.as_ref()?.highest.map(|v| v as f64)
    }
    fn avg_stress(&self) -> Option<f64> {
        self.stress.as_ref()?.avg.map(|v| v as f64)
    }
    fn readiness(&self) -> Option<f64> {
        self.training_readiness.as_ref()?.score.map(|v| v as f64)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Baseline {
    pub sleep_hours: Option<f64>,
    pub sleep_score: Option<f64>,
    pub hrv_last_night_ms: Option<f64>,
    pub resting_hr: Option<f64>,
    pub body_battery_peak: Option<f64>,
    pub avg_stress: Option<f64>,
    pub training_readiness: Option<f64>,
    pub days_with_sleep: usize,
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let v: Vec<f64> = values.collect();
    (!v.is_empty()).then(|| round1(v.iter().sum::<f64>() / v.len() as f64))
}

impl Baseline {
    fn from_days(days: &[DayView]) -> Self {
        Self {
            sleep_hours: mean(days.iter().filter_map(DayView::sleep_hours)),
            sleep_score: mean(days.iter().filter_map(DayView::sleep_score)),
            hrv_last_night_ms: mean(days.iter().filter_map(DayView::hrv)),
            resting_hr: mean(days.iter().filter_map(DayView::resting_hr)),
            body_battery_peak: mean(days.iter().filter_map(DayView::bb_peak)),
            avg_stress: mean(days.iter().filter_map(DayView::avg_stress)),
            training_readiness: mean(days.iter().filter_map(DayView::readiness)),
            days_with_sleep: days.iter().filter(|d| d.sleep.is_some()).count(),
        }
    }
}

/// Today minus baseline, for each metric where both exist.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Comparison {
    pub sleep_hours: Option<f64>,
    pub sleep_score: Option<f64>,
    pub hrv_last_night_ms: Option<f64>,
    pub resting_hr: Option<f64>,
    pub body_battery_peak: Option<f64>,
    pub training_readiness: Option<f64>,
    /// Yesterday's full-day stress against the baseline (today's is incomplete).
    pub avg_stress_yesterday: Option<f64>,
}

impl Comparison {
    fn new(day: &DayView, stress_yesterday: Option<&StressView>, base: &Baseline) -> Self {
        let diff = |a: Option<f64>, b: Option<f64>| Some(round1(a? - b?));
        Self {
            sleep_hours: diff(day.sleep_hours(), base.sleep_hours),
            sleep_score: diff(day.sleep_score(), base.sleep_score),
            hrv_last_night_ms: diff(day.hrv(), base.hrv_last_night_ms),
            resting_hr: diff(day.resting_hr(), base.resting_hr),
            body_battery_peak: diff(day.bb_peak(), base.body_battery_peak),
            training_readiness: diff(day.readiness(), base.training_readiness),
            avg_stress_yesterday: diff(
                stress_yesterday.and_then(|s| s.avg).map(|v| v as f64),
                base.avg_stress,
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub day: DayView,
    pub stress_yesterday: Option<StressView>,
    pub baseline_28d: Baseline,
    pub vs_baseline: Comparison,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct History {
    pub days: Vec<DayView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Replace long arrays (per-minute samples) with a short placeholder so raw
/// responses stay readable and cheap to send to Claude.
pub fn strip_series(value: &mut Value) {
    match value {
        Value::Array(items) if items.len() > 24 => {
            let n = items.len();
            *value = Value::String(format!("<{n} samples omitted; use include_series=true>"));
        }
        Value::Array(items) => items.iter_mut().for_each(strip_series),
        Value::Object(map) => map.values_mut().for_each(strip_series),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_connect::{Endpoints, MemoryTokenStore, Tokens};

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
        fn today(&self) -> NaiveDate {
            self.0.date_naive()
        }
    }

    fn recovery(dir: &std::path::Path, now: &str) -> Recovery {
        let tokens = Tokens { access_token: "x".into(), refresh_token: None, client_id: "c".into() };
        let client = GarminClient::with_endpoints(
            Endpoints::single_host("http://127.0.0.1:9"),
            MemoryTokenStore::with_tokens(tokens),
        )
        .unwrap();
        Recovery::with_clock(Arc::new(client), Store::new(dir), Box::new(FixedClock(now.parse().unwrap())))
    }

    fn rec(date: &str, fetched: &str) -> DayRecord {
        DayRecord {
            date: date.parse().unwrap(),
            fetched_at: fetched.parse().unwrap(),
            summary: Value::Null,
            raw: Value::Null,
        }
    }

    #[test]
    fn freshness_rules() {
        let dir = tempfile::tempdir().unwrap();
        let r = recovery(dir.path(), "2026-11-07T12:00:00Z");
        // Today, fetched 10 minutes ago: fresh.
        assert!(r.is_fresh(&rec("2026-11-07", "2026-11-07T11:50:00Z")));
        // Today, fetched 2 hours ago: stale.
        assert!(!r.is_fresh(&rec("2026-11-07", "2026-11-07T10:00:00Z")));
        // Old day fetched long after it settled: final.
        assert!(r.is_fresh(&rec("2026-10-01", "2026-10-20T10:00:00Z")));
        // Old day fetched on the same day (before it settled): re-fetch once.
        assert!(!r.is_fresh(&rec("2026-10-01", "2026-10-01T08:00:00Z")));
    }

    #[test]
    fn baseline_and_comparison() {
        let day = |hours: f64, score: i64, stress: i64| DayView {
            date: "2026-11-01".parse().unwrap(),
            sleep: Some(SleepView {
                hours: Some(hours),
                score: Some(score),
                score_label: None,
                bedtime: None,
                wake_time: None,
                deep_min: None,
                light_min: None,
                rem_min: None,
                awake_min: None,
                avg_overnight_hrv_ms: Some(60.0),
                body_battery_recharged: None,
                avg_sleep_stress: None,
            }),
            hrv: None,
            training_readiness: None,
            body_battery: None,
            stress: Some(StressView {
                avg: Some(stress),
                max: None,
                label: None,
                rest_min: None,
                low_min: None,
                medium_min: None,
                high_min: None,
                activity_min: None,
            }),
            resting_hr: Some(RestingHrView { bpm: Some(50), avg_7d: None }),
            spo2: None,
            respiration: None,
        };
        let base = Baseline::from_days(&[day(7.0, 70, 20), day(8.0, 80, 30)]);
        assert_eq!(base.sleep_hours, Some(7.5));
        assert_eq!(base.sleep_score, Some(75.0));
        assert_eq!(base.hrv_last_night_ms, Some(60.0));
        assert_eq!(base.resting_hr, Some(50.0));
        assert_eq!(base.avg_stress, Some(25.0));
        assert_eq!(base.training_readiness, None);
        assert_eq!(base.days_with_sleep, 2);

        let today = day(6.0, 65, 99);
        let yesterday = day(7.0, 70, 40).stress;
        let cmp = Comparison::new(&today, yesterday.as_ref(), &base);
        assert_eq!(cmp.sleep_hours, Some(-1.5));
        assert_eq!(cmp.sleep_score, Some(-10.0));
        assert_eq!(cmp.body_battery_peak, None);
        // Uses yesterday's full day, not today's partial stress.
        assert_eq!(cmp.avg_stress_yesterday, Some(15.0));
    }

    #[test]
    fn resting_hr_falls_back_to_sleep_data() {
        let sleep = SleepDay::from_raw(
            "2026-11-07".parse().unwrap(),
            &serde_json::json!({"dailySleepDTO": {"sleepTimeSeconds": 25000}, "restingHeartRate": 47}),
        )
        .unwrap();
        let view = DayView::new(
            "2026-11-07".parse().unwrap(),
            Sources { sleep: Some(&sleep), ..Default::default() },
            false,
        );
        assert_eq!(view.resting_hr.unwrap().bpm, Some(47));
        assert!(view.stress.is_none());
        assert!(view.spo2.is_none());
    }

    #[test]
    fn strips_long_arrays_only() {
        let mut v = serde_json::json!({"short": [1, 2], "long": (0..100).collect::<Vec<_>>()});
        strip_series(&mut v);
        assert_eq!(v["short"], serde_json::json!([1, 2]));
        assert!(v["long"].as_str().unwrap().contains("100 samples"));
    }

    fn fixture(name: &str) -> Value {
        let path = format!("{}/../garmin-connect/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn snapshot_fetches_once_then_uses_the_store() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/userprofile-service/socialProfile"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"displayName": "runner"})),
            )
            .mount(&server)
            .await;
        // 28 baseline days + the day itself = 29 requests, and only on the first call.
        Mock::given(method("GET"))
            .and(path("/wellness-service/wellness/dailySleepData/runner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("sleep.json")))
            .expect(29)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/hrv-service/hrv/\d{4}-\d{2}-\d{2}$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("hrv.json")))
            .expect(29)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/usersummary-service/usersummary/daily/runner"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("daily_summary.json")))
            .expect(29)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/metrics-service/metrics/trainingreadiness/\d{4}-\d{2}-\d{2}$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("training_readiness.json")))
            .expect(29)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wellness-service/wellness/bodyBattery/reports/daily"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture("body_battery.json")))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let tokens = Tokens { access_token: "x".into(), refresh_token: None, client_id: "c".into() };
        let client = GarminClient::with_endpoints(
            Endpoints::single_host(&server.uri()),
            MemoryTokenStore::with_tokens(tokens),
        )
        .unwrap();
        let r = Recovery::with_clock(
            Arc::new(client),
            Store::new(dir.path()),
            Box::new(FixedClock("2026-11-07T12:00:00Z".parse().unwrap())),
        );

        let date: NaiveDate = "2026-11-07".parse().unwrap();
        let first = r.snapshot(date).await.unwrap();
        let sleep = first.day.sleep.as_ref().unwrap();
        assert_eq!(sleep.hours, Some(7.4));
        assert_eq!(sleep.score, Some(81));
        assert_eq!(first.day.hrv.as_ref().unwrap().status.as_deref(), Some("BALANCED"));
        assert_eq!(first.day.body_battery.as_ref().unwrap().highest, Some(88));
        assert_eq!(first.baseline_28d.days_with_sleep, 28);
        assert_eq!(first.vs_baseline.sleep_hours, Some(0.0));
        // Body Battery only had data for the 7th, so the baseline peak is missing.
        assert_eq!(first.baseline_28d.body_battery_peak, None);
        assert!(dir.path().join("sleep/2026-10-10.json").exists());
        assert!(dir.path().join("daily_summary/2026-10-10.json").exists());

        let tr = first.day.training_readiness.as_ref().unwrap();
        assert_eq!(tr.score, Some(68));
        assert_eq!(tr.recovery_time_hours, Some(21.0));
        assert_eq!(tr.factors["hrv"].percent, Some(90));
        assert_eq!(tr.calculated_at.as_deref(), Some("07:35"));
        assert_eq!(first.day.stress.as_ref().unwrap().high_min, Some(22));
        assert_eq!(first.stress_yesterday.as_ref().unwrap().avg, Some(27));
        assert_eq!(first.day.resting_hr.as_ref().unwrap().bpm, Some(49));
        assert_eq!(first.day.spo2.as_ref().unwrap().sleep_avg, Some(95.0));
        assert_eq!(first.day.spo2.as_ref().unwrap().lowest, Some(89));
        assert_eq!(first.day.respiration.as_ref().unwrap().waking_avg, Some(15.0));
        let bb = first.day.body_battery.as_ref().unwrap();
        assert_eq!(bb.at_wake, Some(84));
        let events = bb.events.as_ref().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].impact, Some(-14));
        assert_eq!(events[1].duration_min, Some(59));
        assert_eq!(first.baseline_28d.training_readiness, Some(68.0));
        assert_eq!(first.vs_baseline.avg_stress_yesterday, Some(0.0));

        // History rows leave out the event lists.
        let history = r.history("2026-11-06".parse().unwrap(), date).await.unwrap();
        assert!(history.days.iter().all(|d| d.body_battery.as_ref().unwrap().events.is_none()));

        // Second call within 30 minutes: everything comes from disk.
        let second = r.snapshot(date).await.unwrap();
        assert_eq!(second.day, first.day);
    }
}
