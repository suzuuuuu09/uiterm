//! Isolated herdr-core daemon lifecycle manager.
//!
//! Launches a dedicated herdr-core instance with isolated XDG directories,
//! session name, and Unix socket path so existing user Herdr / herdr-core
//! sessions and configs are never modified.

use crate::herdr::types::IpcError;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::process::Command;

static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct HerdrLaunchConfig {
    pub binary: PathBuf,
    pub session: String,
    pub base_dir: PathBuf,
    pub socket_path: PathBuf,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub cleanup_on_drop: bool,
}

impl HerdrLaunchConfig {
    /// Build an isolated configuration under `/tmp/ut-<pid>-<seq>/` so that
    /// the Unix socket path always stays well under macOS's 104-byte limit.
    pub fn isolated(prefix: &str) -> Result<Self, IpcError> {
        let binary = resolve_herdr_core_bin()?;
        let pid = std::process::id();
        let seq = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base_dir = PathBuf::from(format!("/tmp/ut-{prefix}-{pid}-{seq}"));
        let state_dir = base_dir.join("st");
        let config_dir = base_dir.join("cf");
        let runtime_dir = base_dir.join("rt");
        let socket_path = base_dir.join("hc.sock");
        let session = format!("ut-{prefix}-{pid}-{seq}");

        Ok(Self {
            binary,
            session,
            base_dir,
            socket_path,
            state_dir,
            config_dir,
            runtime_dir,
            cleanup_on_drop: true,
        })
    }

    fn apply_env(&self, cmd: &mut Command) {
        cmd.env("HC_SESSION", &self.session)
            .env("HC_SOCKET", &self.socket_path)
            .env("XDG_STATE_HOME", &self.state_dir)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir);
    }
}

pub fn resolve_herdr_core_bin() -> Result<PathBuf, IpcError> {
    if let Ok(explicit) = std::env::var("HERDR_CORE_BIN") {
        let p = PathBuf::from(explicit);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for candidate in ["herdr-core", "hc"] {
                let p = dir.join(candidate);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }
    Err(IpcError::with_hint(
        "herdr_core_not_found",
        "herdr-core binary not found in HERDR_CORE_BIN or PATH",
        Some("Run inside `nix develop` or set HERDR_CORE_BIN".into()),
    ))
}

#[derive(Debug)]
pub struct HerdrDaemon {
    config: HerdrLaunchConfig,
    stopped: bool,
}

impl HerdrDaemon {
    /// Start an isolated herdr-core daemon and wait until its Unix socket accepts connections.
    pub async fn start(config: HerdrLaunchConfig) -> Result<Self, IpcError> {
        for dir in [
            &config.base_dir,
            &config.state_dir,
            &config.config_dir,
            &config.runtime_dir,
        ] {
            std::fs::create_dir_all(dir).map_err(|e| {
                IpcError::new(
                    "io_error",
                    format!("failed to create {}: {e}", dir.display()),
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }

        let mut cmd = Command::new(&config.binary);
        cmd.args(["server", "start"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        config.apply_env(&mut cmd);

        let output = cmd.output().await.map_err(|e| {
            IpcError::new(
                "spawn_failed",
                format!(
                    "failed to spawn `{}` server start: {e}",
                    config.binary.display()
                ),
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(IpcError::new(
                "core_start_failed",
                format!(
                    "herdr-core server start failed (code {:?}): {}",
                    output.status.code(),
                    stderr.trim()
                ),
            ));
        }

        wait_for_socket(&config.socket_path, Duration::from_secs(5)).await?;

        Ok(Self {
            config,
            stopped: false,
        })
    }

    pub fn config(&self) -> &HerdrLaunchConfig {
        &self.config
    }

    pub fn socket_path(&self) -> &Path {
        &self.config.socket_path
    }

    /// Gracefully stop this isolated herdr-core daemon and remove its temp directory.
    pub async fn stop(&mut self) -> Result<(), IpcError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;

        let mut cmd = Command::new(&self.config.binary);
        cmd.args(["server", "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.config.apply_env(&mut cmd);
        let _ = tokio::time::timeout(Duration::from_secs(5), cmd.status()).await;

        if self.config.cleanup_on_drop {
            let _ = std::fs::remove_dir_all(&self.config.base_dir);
        }
        Ok(())
    }

    /// Synchronous best-effort stop for Drop / app exit handlers.
    pub fn stop_blocking(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;

        let mut cmd = std::process::Command::new(&self.config.binary);
        cmd.args(["server", "stop"])
            .env("HC_SESSION", &self.config.session)
            .env("HC_SOCKET", &self.config.socket_path)
            .env("XDG_STATE_HOME", &self.config.state_dir)
            .env("XDG_CONFIG_HOME", &self.config.config_dir)
            .env("XDG_RUNTIME_DIR", &self.config.runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = cmd.status();

        if self.config.cleanup_on_drop {
            let _ = std::fs::remove_dir_all(&self.config.base_dir);
        }
    }
}

impl Drop for HerdrDaemon {
    fn drop(&mut self) {
        self.stop_blocking();
    }
}

async fn wait_for_socket(path: &Path, timeout: Duration) -> Result<(), IpcError> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() && tokio::net::UnixStream::connect(path).await.is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(IpcError::new(
                "core_start_timeout",
                format!("timed out waiting for herdr-core socket at {}", path.display()),
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
