//! Lifecycle of the app's private herdr-core instance.

use crate::herdr::types::IpcError;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

mod supervisor;
pub(crate) use supervisor::CoreSupervisor;
pub use supervisor::run_supervisor;

static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);
pub(crate) const CORE_VERSION: &str = "0.1.22";
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct HerdrLaunchConfig {
    binary: PathBuf,
    pub session: String,
}

impl HerdrLaunchConfig {
    pub fn isolated(prefix: &str) -> Result<Self, IpcError> {
        let binary = resolve_herdr_core_bin()?;
        let pid = std::process::id();
        let seq = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            binary,
            session: format!("ut-{prefix}-{pid}-{seq}"),
        })
    }
}

pub fn resolve_herdr_core_bin() -> Result<PathBuf, IpcError> {
    if let Some(explicit) = std::env::var_os("HERDR_CORE_BIN") {
        let path = PathBuf::from(explicit);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err(IpcError::new(
            "invalid_core_path",
            "HERDR_CORE_BIN must be an absolute file path",
        ));
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var).filter(|path| path.is_absolute()) {
            for candidate in ["herdr-core", "hc"] {
                let path = dir.join(candidate);
                if path.is_file() {
                    return Ok(path);
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
    base_dir: PathBuf,
    socket_path: PathBuf,
    child: Option<Child>,
    ready: bool,
    stopped: bool,
    stop_attempted: bool,
}

impl HerdrDaemon {
    pub async fn start(config: HerdrLaunchConfig) -> Result<Self, IpcError> {
        let version = tokio::time::timeout(
            STOP_TIMEOUT,
            Command::new(&config.binary)
                .arg("--version")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| IpcError::new("core_start_timeout", "herdr-core version check timed out"))?
        .map_err(|err| IpcError::new("spawn_failed", err.to_string()))?;
        if !version.status.success()
            || String::from_utf8_lossy(&version.stdout).trim() != format!("hc {CORE_VERSION}")
        {
            return Err(IpcError::new(
                "core_version_mismatch",
                "herdr-core v0.1.22 is required",
            ));
        }

        let mut template = *b"/tmp/uiterm.XXXXXXXX\0";
        // mkdtemp creates the directory atomically with mode 0700 and substitutes ASCII.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(IpcError::new(
                "io_error",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let base_dir = PathBuf::from(std::str::from_utf8(&template[..template.len() - 1]).unwrap());
        let mut daemon = Self {
            socket_path: base_dir.join("hc.sock"),
            base_dir,
            config,
            child: None,
            ready: false,
            stopped: false,
            stop_attempted: false,
        };

        // The pinned core's `serve` runs in the foreground and skips shared launchd/systemd
        // startup. We supervise only the process we spawn.
        daemon.spawn_core()?;
        daemon.wait_for_socket(false).await?;
        daemon.ready = true;
        Ok(daemon)
    }

    fn spawn_core(&mut self) -> Result<(), IpcError> {
        self.child = Some(
            Command::from(self.command(&["serve"]))
                .kill_on_drop(true)
                .spawn()
                .map_err(|err| IpcError::new("spawn_failed", err.to_string()))?,
        );
        Ok(())
    }

    async fn wait_for_socket(&mut self, recovering: bool) -> Result<(), IpcError> {
        let deadline = Instant::now() + STOP_TIMEOUT;
        tokio::time::timeout_at(deadline.into(), async {
            loop {
                if tokio::net::UnixStream::connect(self.socket_path())
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                if !recovering
                    && self
                        .child
                        .as_mut()
                        .unwrap()
                        .try_wait()
                        .map_err(stop_error)?
                        .is_some()
                {
                    return Err(IpcError::new(
                        "core_start_failed",
                        "herdr-core exited before opening its socket",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .map_err(|_| IpcError::new("core_start_timeout", "herdr-core did not open its socket"))?
    }

    async fn recover(&mut self) -> Result<(), IpcError> {
        // The holder may win the core lock. A socket in this instance's private
        // directory is the authority, even when the original child has exited.
        if tokio::net::UnixStream::connect(self.socket_path())
            .await
            .is_err()
        {
            self.spawn_core()?;
            self.wait_for_socket(true).await?;
        }
        Ok(())
    }

    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.config.binary);
        cmd.args(args)
            .env_remove("HC_SCOPE")
            .env("HC_RUNTIME", "1")
            .env("HC_LAUNCHD_DIRECT", std::process::id().to_string())
            .env_remove("HC_LAUNCHD_DETACH")
            .env("HC_SESSION", &self.config.session)
            .env("HC_SOCKET", &self.socket_path)
            .env("XDG_STATE_HOME", self.base_dir.join("st"))
            .env("XDG_CONFIG_HOME", self.base_dir.join("cf"))
            // v0.1.22 bypasses the systemd user manager with a private runtime directory.
            .env("XDG_RUNTIME_DIR", self.base_dir.join("rt"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    pub fn config(&self) -> &HerdrLaunchConfig {
        &self.config
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn stop(&mut self) -> Result<(), IpcError> {
        if self.stopped {
            return Ok(());
        }
        self.stop_attempted = true;
        tokio::time::timeout(STOP_TIMEOUT, async {
            self.ensure_graceful_stop_possible()?;
            let output = Command::from(self.command(&["--json", "server", "stop"]))
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .output()
                .await
                .map_err(stop_error)?;
            check_stop_reply(output)?;
            self.child
                .as_mut()
                .unwrap()
                .wait()
                .await
                .map_err(stop_error)?;
            self.remove_state()
        })
        .await
        .map_err(|_| stop_error("graceful stop timed out"))?
    }

    fn ensure_graceful_stop_possible(&self) -> Result<(), IpcError> {
        if !self.socket_path.exists() {
            return Err(stop_error(
                "core socket is missing; preserved state for its holders",
            ));
        }
        Ok(())
    }

    fn remove_state(&mut self) -> Result<(), IpcError> {
        std::fs::remove_dir_all(&self.base_dir)
            .map_err(|err| IpcError::new("io_error", err.to_string()))?;
        self.stopped = true;
        Ok(())
    }

    fn stop_sync(&mut self) -> Result<(), IpcError> {
        if self.stopped {
            return Ok(());
        }
        if !self.ready {
            if let Some(child) = &mut self.child {
                let _ = child.start_kill();
            }
            return self.remove_state();
        }
        self.ensure_graceful_stop_possible()?;
        let mut child = self
            .command(&["--json", "server", "stop"])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(stop_error)?;
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            if child.try_wait().map_err(stop_error)?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(stop_error("graceful stop timed out"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        check_stop_reply(child.wait_with_output().map_err(stop_error)?)?;
        loop {
            if self
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(stop_error)?
                .is_some()
            {
                return self.remove_state();
            }
            if Instant::now() >= deadline {
                return Err(stop_error("core did not exit after accepting stop"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn stop_blocking(&mut self) {
        if !self.stop_attempted && self.stop_sync().is_err() {
            eprintln!("uiterm: core_stop_failed");
        }
    }
}

impl Drop for HerdrDaemon {
    fn drop(&mut self) {
        self.stop_blocking();
    }
}

fn stop_error(err: impl std::fmt::Display) -> IpcError {
    IpcError::new("core_stop_failed", err.to_string())
}

fn check_stop_reply(output: std::process::Output) -> Result<(), IpcError> {
    if !output.status.success() {
        return Err(stop_error(output.status));
    }
    if serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|reply| reply["stopped"].as_bool())
        != Some(true)
    {
        return Err(stop_error(
            "core did not confirm stop; preserved state for its holders",
        ));
    }
    Ok(())
}
