use super::{HerdrDaemon, HerdrLaunchConfig, STOP_TIMEOUT, stop_error};
use crate::herdr::{HerdrClient, IpcError};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

// The supervisor owns the daemon. The GUI keeps the only write end of its
// stdin pipe, so even SIGKILL closes the lifetime handle without PID polling.
pub(crate) struct CoreSupervisor {
    child: Child,
    socket_path: PathBuf,
    session: String,
}

impl CoreSupervisor {
    pub async fn start() -> Result<Self, IpcError> {
        let executable = std::env::current_exe()
            .map_err(|_| IpcError::new("spawn_failed", "cannot locate uiterm executable"))?;
        let mut child = Command::new(executable)
            .arg("--uiterm-core-supervisor")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|err| IpcError::new("spawn_failed", err.to_string()))?;

        let stdout = child.stdout.take().unwrap();
        let mut reply = String::new();
        tokio::time::timeout(STOP_TIMEOUT * 2 + Duration::from_secs(1), async {
            BufReader::new(stdout)
                .take(4096)
                .read_line(&mut reply)
                .await
        })
        .await
        .map_err(|_| IpcError::new("core_start_timeout", "core supervisor did not become ready"))?
        .map_err(|_| IpcError::new("core_start_failed", "cannot read core supervisor reply"))?;
        let (socket_path, session): (PathBuf, String) =
            serde_json::from_str::<Result<_, IpcError>>(&reply).map_err(|_| {
                IpcError::new("core_start_failed", "core supervisor exited during startup")
            })??;

        Ok(Self {
            child,
            socket_path,
            session,
        })
    }

    pub async fn connect(&self) -> Result<HerdrClient, IpcError> {
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            match HerdrClient::connect(&self.socket_path, &self.session).await {
                Err(err) if err.code == "connect_failed" && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                result => return result,
            }
        }
    }

    pub fn stop_blocking(&mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + STOP_TIMEOUT * 3;
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

pub async fn run_supervisor() -> Result<(), IpcError> {
    let started = async {
        let config = HerdrLaunchConfig::isolated("app")?;
        HerdrDaemon::start(config).await
    }
    .await;
    let ready = match &started {
        Ok(daemon) => Ok((daemon.socket_path(), &daemon.config.session)),
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
        tokio::select! {
            _ = &mut gui_exit => {}
            _ = daemon.child.as_mut().unwrap().wait() => {
                if daemon.recover().await.is_err() {
                    eprintln!("uiterm: core_recovery_failed");
                }
                let _ = gui_exit.await;
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
