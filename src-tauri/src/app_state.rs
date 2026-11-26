use crate::herdr::{HerdrClient, IpcError, process::CoreSupervisor};
use std::sync::atomic::{AtomicBool, Ordering};
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
            let mut started = CoreSupervisor::start().await?;
            if self.exiting.load(Ordering::Acquire) {
                drop(connection);
                let _ = tokio::task::spawn_blocking(move || started.stop_blocking()).await;
                return Err(IpcError::new(
                    "core_disconnected",
                    "uiterm is shutting down",
                ));
            }
            connection.supervisor = Some(started);
        }
        let supervisor = connection.supervisor.as_ref().unwrap();
        let client = supervisor.connect().await?;
        if self.exiting.load(Ordering::Acquire) {
            let supervisor = connection.supervisor.take();
            drop(connection);
            if let Some(mut supervisor) = supervisor {
                let _ = tokio::task::spawn_blocking(move || supervisor.stop_blocking()).await;
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
