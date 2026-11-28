use crate::herdr::{
    HerdrClient, IpcError,
    process::{CoreSupervisor, STOP_TIMEOUT},
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Default)]
pub(crate) struct AppState {
    connection: Mutex<Connection>,
    exiting: AtomicBool,
}

#[derive(Default)]
struct Connection {
    supervisor: Option<CoreSupervisor>,
    client: Option<HerdrClient>,
}

impl AppState {
    pub async fn ensure_client(&self) -> Result<HerdrClient, IpcError> {
        let mut connection = self.connection.lock().await;
        if self.exiting.load(Ordering::Acquire) {
            return Err(IpcError::new(
                "core_disconnected",
                "uiterm is shutting down",
            ));
        }
        if let Some(client) = &connection.client
            && client.is_connected()
        {
            return Ok(client.clone());
        }

        if connection.supervisor.is_none() {
            let executable = std::env::current_exe()
                .map_err(|_| IpcError::new("spawn_failed", "cannot locate uiterm executable"))?;
            let mut started = CoreSupervisor::start(&executable).await?;
            if self.exiting.load(Ordering::Acquire) {
                drop(connection);
                if let Err(err) = tokio::task::spawn_blocking(move || started.stop_blocking()).await
                {
                    eprintln!("uiterm: core_supervisor_stop_failed: {err}");
                }
                return Err(IpcError::new(
                    "core_disconnected",
                    "uiterm is shutting down",
                ));
            }
            connection.supervisor = Some(started);
        }
        let supervisor = connection.supervisor.as_ref().unwrap();
        let (socket_path, session) = supervisor.endpoint();
        let deadline = Instant::now() + STOP_TIMEOUT;
        let client = loop {
            match HerdrClient::connect(socket_path, session).await {
                Err(err) if err.code == "connect_failed" && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                result => break result?,
            }
        };
        if self.exiting.load(Ordering::Acquire) {
            let supervisor = connection.supervisor.take();
            drop(connection);
            if let Some(mut supervisor) = supervisor
                && let Err(err) =
                    tokio::task::spawn_blocking(move || supervisor.stop_blocking()).await
            {
                eprintln!("uiterm: core_supervisor_stop_failed: {err}");
            }
            return Err(IpcError::new(
                "core_disconnected",
                "uiterm is shutting down",
            ));
        }
        connection.client = Some(client.clone());
        Ok(client)
    }

    pub fn shutdown_blocking(&self) {
        self.exiting.store(true, Ordering::Release);
        if let Ok(mut connection) = self.connection.try_lock()
            && let Some(mut supervisor) = connection.supervisor.take()
        {
            connection.client = None;
            supervisor.stop_blocking();
        }
    }
}
