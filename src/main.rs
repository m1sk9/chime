mod config;
mod heartbeat;
mod notifier;
mod runtime;
mod scheduler;

use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::config::{Config, LogLevel};
use crate::heartbeat::{check_liveness, heartbeat_path};
use crate::notifier::Discord;
use crate::runtime::resolve;
use crate::scheduler::Scheduler;

const DEFAULT_CONFIG_PATH: &str = "/etc/chime/config.toml";

enum Command {
    /// Run the scheduler daemon (no arguments).
    Daemon,
    /// Exit 0 if the scheduler is ticking (heartbeat fresh), non-zero otherwise.
    Health,
}

fn parse_command() -> Result<Command, String> {
    let mut args = std::env::args().skip(1); // skip argv[0]
    match args.next().as_deref() {
        None => Ok(Command::Daemon),
        Some("health") => {
            if args.next().is_some() {
                return Err("`health` takes no arguments".to_string());
            }
            Ok(Command::Health)
        }
        Some(other) => Err(format!("unknown argument: {other}")),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let command = match parse_command() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: {e}");
            std::process::exit(2);
        }
    };
    let result = match command {
        Command::Daemon => run_daemon().await,
        Command::Health => run_health(),
    };
    if let Err(e) = result {
        eprintln!("fatal: {e:#}");
        std::process::exit(1);
    }
}

fn load_config() -> Result<Config> {
    let path = std::env::var("CHIME_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file at {path}"))?;
    Config::from_toml(&text).context("failed to parse config")
}

/// Liveness check used by `HEALTHCHECK` / `healthcheck:`. Reads the heartbeat
/// file's mtime and compares it against `2 * tick_interval`. This answers "is the
/// scheduler ticking?", not "did the last Discord send succeed?".
fn run_health() -> Result<()> {
    let cfg = load_config()?;
    let interval = cfg.system.tick_interval_sec.as_duration();
    let path = heartbeat_path();
    match check_liveness(&path, interval, SystemTime::now()) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn run_daemon() -> Result<()> {
    let cfg = load_config()?;
    let run_cfg = resolve(cfg).context("failed to resolve runtime config")?;

    init_logging(run_cfg.log_level);

    info!(
        reminders = run_cfg.reminders.len(),
        interval_sec = run_cfg.interval.as_secs(),
        timezone = %run_cfg.timezone,
        "chime starting"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client")?;
    let notifier = Discord::new(client);
    let scheduler = Scheduler::new(run_cfg, notifier);
    scheduler
        .run()
        .await
        .context("scheduler exited with error")?;
    Ok(())
}

fn init_logging(level: LogLevel) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let directive = match level {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        EnvFilter::new(directive)
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json().with_current_span(false))
        .try_init();
}
