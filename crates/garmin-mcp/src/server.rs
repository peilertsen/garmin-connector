//! The MCP server Claude talks to.

use std::sync::Arc;

use chrono::NaiveDate;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::recovery::{MAX_RANGE_DAYS, Recovery, strip_series};
use crate::store::Metric;

const INSTRUCTIONS: &str = "\
Read-only access to the user's Garmin sleep, HRV and Body Battery data. \
Start with get_recovery_snapshot for a morning check-in: it returns last night's \
numbers next to the user's own 28-day baseline. Use get_recovery_history for trends \
and correlations. Dates are the day the user woke up. Training data is not here; \
it comes from Strava. These are wellness numbers from a consumer watch, not medical \
measurements.";

#[derive(Clone)]
pub struct GarminServer {
    recovery: Arc<Recovery>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SnapshotParams {
    /// Day to report on, YYYY-MM-DD (the day the user woke up). Defaults to today.
    pub date: Option<NaiveDate>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryParams {
    /// First day, YYYY-MM-DD.
    pub start_date: NaiveDate,
    /// Last day, YYYY-MM-DD (inclusive). Defaults to today.
    pub end_date: Option<NaiveDate>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RawParams {
    /// Which Garmin dataset: sleep, hrv or body_battery.
    pub metric: Metric,
    /// Day, YYYY-MM-DD. Defaults to today.
    pub date: Option<NaiveDate>,
    /// Include per-minute sample arrays (large). Default false.
    #[serde(default)]
    pub include_series: bool,
}

fn json_result(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    let text =
        serde_json::to_string_pretty(value).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Report failures to Claude as tool errors it can explain to the user,
/// rather than protocol errors.
fn tool_error(e: anyhow::Error) -> Result<CallToolResult, ErrorData> {
    let mut msg = format!("{e:#}");
    if msg.contains("not logged in") {
        msg.push_str(". Ask the user to run `garmin-mcp login` in a terminal.");
    }
    Ok(CallToolResult::error(vec![ContentBlock::text(msg)]))
}

#[tool_router]
impl GarminServer {
    pub fn new(recovery: Arc<Recovery>) -> Self {
        Self { recovery, tool_router: Self::tool_router() }
    }

    #[tool(description = "Last night's sleep (duration, score, stages, bedtime, resting HR), HRV \
                       (last night, weekly average, status, personal balanced range) and Body \
                       Battery (current, peak, low), compared with the user's 28-day baseline. \
                       Use this for daily briefings and 'how did I recover' questions.")]
    async fn get_recovery_snapshot(
        &self,
        Parameters(p): Parameters<SnapshotParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = p.date.unwrap_or_else(|| self.recovery.today());
        match self.recovery.snapshot(date).await {
            Ok(s) => json_result(&s),
            Err(e) => tool_error(e),
        }
    }

    #[tool(description = "One row per day of sleep, HRV and Body Battery summaries for a date range \
                       (at most 92 days). Use for trends, weekly reviews, and looking for patterns, \
                       e.g. comparing nights before headaches or hard training days.")]
    async fn get_recovery_history(
        &self,
        Parameters(p): Parameters<HistoryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let end = p.end_date.unwrap_or_else(|| self.recovery.today());
        if (end - p.start_date).num_days() >= MAX_RANGE_DAYS {
            return tool_error(anyhow::anyhow!(
                "range too long: request at most {MAX_RANGE_DAYS} days at a time"
            ));
        }
        match self.recovery.history(p.start_date, end).await {
            Ok(h) => json_result(&h),
            Err(e) => tool_error(e),
        }
    }

    #[tool(description = "Garmin's original JSON for one dataset and day, for fields the summaries \
                       don't include (e.g. sleep sub-scores, SpO2 details, Body Battery events). \
                       Long per-minute arrays are omitted unless include_series is true.")]
    async fn get_garmin_raw(
        &self,
        Parameters(p): Parameters<RawParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = p.date.unwrap_or_else(|| self.recovery.today());
        match self.recovery.raw(p.metric, date).await {
            Ok(mut raw) => {
                if !p.include_series {
                    strip_series(&mut raw);
                }
                json_result(&raw)
            }
            Err(e) => tool_error(e),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GarminServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).with_title("Garmin"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}
