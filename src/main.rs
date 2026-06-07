mod config;
mod notifier;
mod runtime;
mod scheduler;

use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::config::{Config, LogLevel};
use crate::notifier::Discord;
use crate::runtime::resolve;
use crate::scheduler::Scheduler;

const DEFAULT_CONFIG_PATH: &str = "/etc/chime/config.toml";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("fatal: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let path = std::env::var("CHIME_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file at {path}"))?;
    let cfg = Config::from_toml(&text).context("failed to parse config")?;
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
