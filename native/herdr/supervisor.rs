use super::CoreProcessError;
use super::{HerdrDaemon, HerdrLaunchConfig, STOP_TIMEOUT, stop_error};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

// The supervisor owns the daemon. The GUI keeps the only write end of its
// stdin pipe, so even SIGKILL closes the lifetime handle without PID polling.
pub const SUPERVISOR_FLAG: &str = "--uiterm-core-supervisor";

pub(crate) struct CoreSupervisor {
    child: Child,
    socket_path: PathBuf,
    session: String,
}

impl CoreSupervisor {
    pub async fn start(executable: &Path) -> Result<Self, CoreProcessError> {
        let mut child = Command::new(executable)
            .arg(SUPERVISOR_FLAG)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|err| CoreProcessError::new("spawn_failed", err.to_string()))?;

        let stdout = child.stdout.take().unwrap();
        let mut reply = String::new();
        tokio::time::timeout(STOP_TIMEOUT * 2 + Duration::from_secs(1), async {
            BufReader::new(stdout)
                .take(4096)
                .read_line(&mut reply)
                .await
        })
        .await
        .map_err(|_| {
            CoreProcessError::new("core_start_timeout", "core supervisor did not become ready")
        })?
        .map_err(|_| {
            CoreProcessError::new("core_start_failed", "cannot read core supervisor reply")
        })?;
        let (socket_path, session): (PathBuf, String) =
            serde_json::from_str::<Result<_, CoreProcessError>>(&reply).map_err(|_| {
                CoreProcessError::new("core_start_failed", "core supervisor exited during startup")
            })??;

        Ok(Self {
            child,
            socket_path,
            session,
        })
    }

    pub fn endpoint(&self) -> (&Path, &str) {
        (&self.socket_path, &self.session)
    }

    pub fn stop_blocking(&mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + STOP_TIMEOUT * 3;
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            eprintln!("uiterm: core_supervisor_stop_timeout");
            #[cfg(unix)]
            if let Some(pid) = self.child.id()
                && let Ok(pgid) = i32::try_from(pid)
                && pgid > 0
            {
                // Supervisor was spawned with process_group(0), so its PID is its PGID.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
            let _ = self.child.start_kill();
        }
    }
}

impl Drop for CoreSupervisor {
    fn drop(&mut self) {
        self.stop_blocking();
    }
}

pub async fn run_supervisor() -> Result<(), CoreProcessError> {
    let started = async {
        let config = HerdrLaunchConfig::isolated("app")?;
        HerdrDaemon::start(config).await
    }
    .await;
    let ready = match &started {
        Ok(daemon) => Ok((daemon.socket_path(), &daemon.config().session)),
        Err(err) => Err(err),
    };
    let announcement = writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string(&ready).unwrap()
    );
    let mut daemon = started?;
    if announcement.is_ok() {
        let mut gui_exit = tokio::task::spawn_blocking(|| {
            let mut byte = [0; 1];
            std::io::Read::read(&mut std::io::stdin(), &mut byte)
        });
        let mut backoff = Duration::from_millis(250);
        let mut last_recovery_err: Option<String> = None;
        loop {
            let child_alive = matches!(daemon.child.as_mut().unwrap().try_wait(), Ok(None));
            if child_alive {
                tokio::select! {
                    _ = &mut gui_exit => break,
                    _ = daemon.child.as_mut().unwrap().wait() => {
                        match daemon.recover().await {
                            Ok(()) => {
                                backoff = Duration::from_millis(250);
                                last_recovery_err = None;
                            }
                            Err(err) => {
                                let summary = format!("[{}] {}", err.code, err.message);
                                if last_recovery_err.as_deref() != Some(summary.as_str()) {
                                    eprintln!("uiterm: core_recovery_failed: {summary}");
                                    last_recovery_err = Some(summary);
                                }
                                tokio::select! {
                                    _ = &mut gui_exit => break,
                                    _ = tokio::time::sleep(backoff) => {}
                                }
                                backoff = (backoff * 2).min(Duration::from_secs(5));
                            }
                        }
                    }
                }
            } else {
                // The tracked child already exited (e.g. a holder adopted the lock or a
                // prior recovery failed). Poll recovery with backoff without spinning.
                tokio::select! {
                    _ = &mut gui_exit => break,
                    _ = tokio::time::sleep(backoff) => {
                        match daemon.recover().await {
                            Ok(()) => {
                                backoff = Duration::from_millis(250);
                                last_recovery_err = None;
                            }
                            Err(err) => {
                                let summary = format!("[{}] {}", err.code, err.message);
                                if last_recovery_err.as_deref() != Some(summary.as_str()) {
                                    eprintln!("uiterm: core_recovery_failed: {summary}");
                                    last_recovery_err = Some(summary);
                                }
                                backoff = (backoff * 2).min(Duration::from_secs(5));
                            }
                        }
                    }
                }
            }
        }
    }

    if daemon
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .map_err(stop_error)?
        .is_some()
    {
        daemon.recover().await?;
    }
    // If the core died while accepting stop, the replacement must adopt
    // the remaining holders before we can remove their state.
    match daemon.stop().await {
        Err(_)
            if daemon
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(stop_error)?
                .is_some() =>
        {
            daemon.recover().await?;
            daemon.stop().await
        }
        result => result,
    }
}
