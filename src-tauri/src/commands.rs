//! Tauri 2 IPC Commands & Channels implementing docs/poc/interface.md.

use crate::config_runtime::{self, ConfigEvalParams, ConfigEvalResult};
use crate::herdr::{
    CoreEvent, CoreStatus, DisplayFacts, EventsSubscribeResult, HerdrClient, HerdrDaemon,
    HerdrLaunchConfig, IpcError, TerminalAttachParams, TerminalAttachResult, TerminalCreateParams,
    TerminalInfo,
};
use std::sync::Arc;
use tauri::State;
use tauri::ipc::{Channel, InvokeBody, Request, Response};
use tokio::sync::Mutex;

pub struct AppState {
    pub daemon: Mutex<Option<HerdrDaemon>>,
    pub client: Mutex<Option<HerdrClient>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            daemon: Mutex::new(None),
            client: Mutex::new(None),
        }
    }

    pub async fn ensure_client(&self) -> Result<HerdrClient, IpcError> {
        {
            let guard = self.client.lock().await;
            if let Some(client) = guard.as_ref() {
                return Ok(client.clone());
            }
        }
        self.connect_fresh().await
    }

    pub async fn connect_fresh(&self) -> Result<HerdrClient, IpcError> {
        let (socket_path, session) = {
            let mut daemon_guard = self.daemon.lock().await;
            if daemon_guard.is_none() {
                let cfg = HerdrLaunchConfig::isolated("app")?;
                let daemon = HerdrDaemon::start(cfg).await?;
                *daemon_guard = Some(daemon);
            }
            let d = daemon_guard.as_ref().unwrap();
            (d.socket_path().to_path_buf(), d.config().session.clone())
        };

        let client = HerdrClient::connect(&socket_path, session).await?;
        let mut guard = self.client.lock().await;
        *guard = Some(client.clone());
        Ok(client)
    }

    pub fn shutdown_blocking(&self) {
        if let Ok(mut guard) = self.daemon.try_lock()
            && let Some(daemon) = guard.as_mut()
        {
            daemon.stop_blocking();
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub async fn core_status(state: State<'_, Arc<AppState>>) -> Result<CoreStatus, IpcError> {
    let client = state.ensure_client().await?;
    client.server_status().await
}

#[tauri::command]
pub async fn core_reconnect(state: State<'_, Arc<AppState>>) -> Result<CoreStatus, IpcError> {
    let client = state.connect_fresh().await?;
    client.server_status().await
}

#[tauri::command]
pub async fn terminal_create(
    state: State<'_, Arc<AppState>>,
    params: TerminalCreateParams,
) -> Result<TerminalInfo, IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_create(params).await
}

#[tauri::command]
pub async fn terminal_list(
    state: State<'_, Arc<AppState>>,
    all: bool,
) -> Result<Vec<TerminalInfo>, IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_list(all).await
}

#[tauri::command]
pub async fn terminal_attach(
    state: State<'_, Arc<AppState>>,
    params: TerminalAttachParams,
    on_output: Channel<Response>,
) -> Result<TerminalAttachResult, IpcError> {
    let client = state.ensure_client().await?;
    let mut handle = client.terminal_attach(params).await?;
    let result = handle.result.clone();

    tokio::spawn(async move {
        while let Some(frame) = handle.rx.recv().await {
            let ipc_bytes = frame.into_ipc_bytes();
            if on_output.send(Response::new(ipc_bytes)).is_err() {
                break;
            }
        }
    });

    Ok(result)
}

#[tauri::command]
pub async fn terminal_detach(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_detach(attach_id).await
}

#[tauri::command]
pub async fn terminal_write(
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Result<(), IpcError> {
    let attach_id = request
        .headers()
        .get("x-uiterm-attach-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(|| {
            IpcError::new(
                "invalid_args",
                "missing or invalid header x-uiterm-attach-id",
            )
        })?;

    let automatic = request
        .headers()
        .get("x-uiterm-input-kind")
        .and_then(|v| v.to_str().ok())
        .map(|s| s == "automatic")
        .unwrap_or(false);

    let bytes = match request.body() {
        InvokeBody::Raw(raw) => raw.clone(),
        InvokeBody::Json(val) => {
            if let Some(s) = val.as_str() {
                s.as_bytes().to_vec()
            } else {
                return Err(IpcError::new(
                    "invalid_args",
                    "terminal_write expects raw binary body (Uint8Array)",
                ));
            }
        }
    };

    let client = state.ensure_client().await?;
    client.terminal_write(attach_id, bytes, automatic).await
}

#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_resize(attach_id, cols, rows).await
}

#[tauri::command]
pub async fn terminal_focus(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    focused: bool,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_focus(attach_id, focused).await
}

#[tauri::command]
pub async fn terminal_facts(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    display: DisplayFacts,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_facts(attach_id, display).await
}

#[tauri::command]
pub async fn terminal_ack(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    bytes: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_ack(attach_id, bytes).await
}

#[tauri::command]
pub async fn terminal_kill(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_kill(&session_id).await
}

#[tauri::command]
pub async fn events_subscribe(
    state: State<'_, Arc<AppState>>,
    on_event: Channel<CoreEvent>,
) -> Result<EventsSubscribeResult, IpcError> {
    let client = state.ensure_client().await?;
    let (result, mut rx) = client.events_subscribe().await?;

    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if on_event.send(ev).is_err() {
                break;
            }
        }
    });

    Ok(result)
}

#[tauri::command]
pub async fn events_unsubscribe(
    state: State<'_, Arc<AppState>>,
    subscription_id: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.events_unsubscribe(subscription_id).await
}

#[tauri::command]
pub async fn config_eval_poc(params: ConfigEvalParams) -> Result<ConfigEvalResult, IpcError> {
    config_runtime::eval_config_poc(params).await
}
