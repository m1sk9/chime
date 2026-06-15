use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const DEFAULT_HEARTBEAT_PATH: &str = "/tmp/chime.heartbeat";
pub const HEARTBEAT_PATH_ENV: &str = "CHIME_HEARTBEAT_PATH";

/// Resolve the heartbeat file path from `CHIME_HEARTBEAT_PATH`, falling back to
/// [`DEFAULT_HEARTBEAT_PATH`]. Shared by the daemon (writer) and `health` (reader).
pub fn heartbeat_path() -> PathBuf {
    std::env::var_os(HEARTBEAT_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_HEARTBEAT_PATH))
}

#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    #[error("heartbeat file {path} does not exist")]
    Missing { path: String },
    #[error("failed to stat heartbeat file {path}: {source}")]
    Stat {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("heartbeat is stale: last update {age_secs}s ago, threshold {threshold_secs}s")]
    Stale { age_secs: u64, threshold_secs: u64 },
    #[error("heartbeat mtime is in the future (clock skew?)")]
    FutureMtime,
}

/// Liveness check: the heartbeat is considered alive when its mtime is no older
/// than `2 * interval`. `now` is injected so the comparison is deterministic in tests.
pub fn check_liveness(path: &Path, interval: Duration, now: SystemTime) -> Result<(), HealthError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            HealthError::Missing {
                path: path.display().to_string(),
            }
        } else {
            HealthError::Stat {
                path: path.display().to_string(),
                source: e,
            }
        }
    })?;
    let mtime = meta.modified().map_err(|source| HealthError::Stat {
        path: path.display().to_string(),
        source,
    })?;
    let threshold = interval.saturating_mul(2);
    let age = now
        .duration_since(mtime)
        .map_err(|_| HealthError::FutureMtime)?;
    if age <= threshold {
        Ok(())
    } else {
        Err(HealthError::Stale {
            age_secs: age.as_secs(),
            threshold_secs: threshold.as_secs(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("chime-test-hb-{}-{}", tag, std::process::id()));
        path
    }

    #[test]
    fn fresh_is_ok() {
        let path = temp_path("fresh");
        std::fs::write(&path, "x").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let interval = Duration::from_secs(30);
        let now = mtime + Duration::from_secs(1);
        assert!(check_liveness(&path, interval, now).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn boundary_is_ok() {
        // age == 2 * interval is still considered alive (<= comparison).
        let path = temp_path("boundary");
        std::fs::write(&path, "x").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let interval = Duration::from_secs(30);
        let now = mtime + Duration::from_secs(60);
        assert!(check_liveness(&path, interval, now).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_errors() {
        let path = temp_path("stale");
        std::fs::write(&path, "x").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let interval = Duration::from_secs(30);
        let now = mtime + Duration::from_secs(61);
        assert!(matches!(
            check_liveness(&path, interval, now),
            Err(HealthError::Stale { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_errors() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            check_liveness(&path, Duration::from_secs(30), SystemTime::now()),
            Err(HealthError::Missing { .. })
        ));
    }

    #[test]
    fn future_mtime_errors() {
        // `now` earlier than the mtime: clock skew, treated as not-alive.
        let path = temp_path("future");
        std::fs::write(&path, "x").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let now = mtime - Duration::from_secs(10);
        assert!(matches!(
            check_liveness(&path, Duration::from_secs(30), now),
            Err(HealthError::FutureMtime)
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn heartbeat_path_default_and_override() {
        // One test so the shared env key is never raced by parallel cases.
        {
            let _g = EnvGuard::unset(HEARTBEAT_PATH_ENV);
            assert_eq!(heartbeat_path(), PathBuf::from(DEFAULT_HEARTBEAT_PATH));
        }
        {
            let _g = EnvGuard::set(HEARTBEAT_PATH_ENV, "/custom/chime.hb");
            assert_eq!(heartbeat_path(), PathBuf::from("/custom/chime.hb"));
        }
    }

    struct EnvGuard {
        key: String,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            EnvGuard {
                key: key.to_string(),
                previous,
            }
        }

        fn unset(key: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            EnvGuard {
                key: key.to_string(),
                previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous.take() {
                    Some(v) => std::env::set_var(&self.key, v),
                    None => std::env::remove_var(&self.key),
                }
            }
        }
    }
}
