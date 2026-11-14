//! `garmin-mcp`: log in to Garmin once, then serve your sleep, HRV and
//! Body Battery data to Claude over MCP.

mod recovery;
mod server;
mod store;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate};
use clap::{Parser, Subcommand};
use garmin_connect::{Endpoints, FileTokenStore, GarminClient, MfaMethod};
use rmcp::ServiceExt;

use crate::recovery::Recovery;
use crate::store::Store;

#[derive(Parser)]
#[command(
    version,
    about = "Garmin sleep, HRV, Body Battery, stress and Training Readiness for Claude (unofficial)"
)]
struct Cli {
    /// Folder for login tokens and stored data.
    /// Defaults to ~/Library/Application Support/garmin-connector on macOS.
    #[arg(long, env = "GARMIN_CONNECTOR_HOME", global = true)]
    home: Option<PathBuf>,

    /// Use garmin.cn (accounts registered in China).
    #[arg(long, env = "GARMIN_CONNECTOR_CHINA", global = true)]
    china: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server on stdin/stdout (what Claude Desktop starts). Default.
    Serve,
    /// Log in to Garmin with email and password. Only tokens are saved.
    Login {
        /// Garmin account email. Asked for if not given.
        #[arg(long, env = "GARMIN_EMAIL")]
        email: Option<String>,
    },
    /// Delete the saved login tokens.
    Logout,
    /// Show where data is kept and whether you're logged in.
    Status,
    /// Print the recovery snapshot Claude would see (handy for testing).
    Today {
        /// Day to show, YYYY-MM-DD. Defaults to today.
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Download the last N days into the local history.
    Sync {
        #[arg(long, default_value_t = 28)]
        days: i64,
    },
    /// Print the raw JSON of any connectapi endpoint (for debugging and
    /// adding new metrics). `{name}` in the path is replaced by your display name.
    #[command(hide = true)]
    DebugGet {
        /// Path, e.g. /metrics-service/metrics/trainingreadiness/2026-11-14
        path: String,
        /// Query parameters as key=value.
        #[arg(long = "query", short = 'q')]
        query: Vec<String>,
    },
}

struct Paths {
    home: PathBuf,
}

impl Paths {
    fn resolve(arg: Option<PathBuf>) -> Result<Self> {
        let home = match arg {
            Some(p) => p,
            None => dirs::data_dir()
                .context("no data directory on this system; pass --home")?
                .join("garmin-connector"),
        };
        Ok(Self { home })
    }
    fn tokens(&self) -> PathBuf {
        self.home.join("tokens.json")
    }
    fn data(&self) -> PathBuf {
        self.home.join("data")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr: stdout carries the MCP protocol.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,garmin_mcp=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let paths = Paths::resolve(cli.home)?;
    let endpoints = if cli.china { Endpoints::china() } else { Endpoints::global() };
    let client = Arc::new(GarminClient::with_endpoints(endpoints, FileTokenStore::new(paths.tokens()))?);
    let recovery = || Arc::new(Recovery::new(client.clone(), Store::new(paths.data())));

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            tracing::info!(home = %paths.home.display(), "starting MCP server");
            let service = server::GarminServer::new(recovery()).serve(rmcp::transport::stdio()).await?;
            service.waiting().await?;
        }
        Command::Login { email } => login(&client, email).await?,
        Command::Logout => {
            client.logout().await?;
            println!("Logged out. Saved tokens deleted.");
        }
        Command::Status => {
            println!("Data folder: {}", paths.home.display());
            if client.has_tokens().await? {
                match client.display_name().await {
                    Ok(_) => println!("Logged in to Garmin: yes, and the API accepts the token."),
                    Err(e) => println!("Logged in to Garmin: tokens saved, but the API refused them: {e}"),
                }
            } else {
                println!("Logged in to Garmin: no. Run `garmin-mcp login`.");
            }
        }
        Command::Today { date } => {
            let r = recovery();
            let date = date.unwrap_or_else(|| r.today());
            let snapshot = r.snapshot(date).await?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        Command::Sync { days } => {
            let r = recovery();
            let end = r.today();
            let start = end - Duration::days(days.max(1) - 1);
            eprintln!("Downloading {start} to {end}...");
            let warnings = r.ensure(start, end).await?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            println!("Done. Data is in {}", paths.data().display());
        }
        Command::DebugGet { path, query } => {
            let name = if path.contains("{name}") { client.display_name().await? } else { String::new() };
            let path = path.replace("{name}", &name);
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            let query: Vec<(&str, String)> = query
                .iter()
                .map(|kv| {
                    kv.split_once('=').map(|(k, v)| (k, v.to_string())).context("query must be key=value")
                })
                .collect::<Result<_>>()?;
            match client.get_json(&segments, &query).await? {
                Some(v) => println!("{}", serde_json::to_string_pretty(&v)?),
                None => println!("(no content)"),
            }
        }
    }
    Ok(())
}

async fn login(client: &GarminClient, email: Option<String>) -> Result<()> {
    let email = match email {
        Some(e) => e,
        None => prompt("Garmin email: ")?,
    };
    // Read from the terminal without echo. GARMIN_PASSWORD is accepted for
    // automation, but typing it is safer (it doesn't land in shell history).
    let password = match std::env::var("GARMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => rpassword::prompt_password("Garmin password (not shown, not saved): ")?,
    };

    eprintln!("Logging in (this takes a few seconds)...");
    client
        .login(&email, &password, |method| {
            let via = match method {
                MfaMethod::Email => "sent to your email",
                MfaMethod::Sms => "sent by SMS",
                MfaMethod::Other(_) => "from your authenticator app",
            };
            prompt(&format!("Two-factor code ({via}): "))
        })
        .await
        .context("login failed")?;
    println!("Logged in as {}. Tokens saved; your password was not.", client.display_name().await?);
    Ok(())
}

fn prompt(label: &str) -> std::io::Result<String> {
    eprint!("{label}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
