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
        self.connect(false).await
    }

    pub async fn connect_fresh(&self) -> Result<HerdrClient, IpcError> {
        self.connect(true).await
    }

    async fn connect(&self, reconnect: bool) -> Result<HerdrClient, IpcError> {
        let mut guard = self.client.lock().await;
        if let Some(client) = guard.as_ref()
            && (client.is_connected() || !reconnect)
        {
            return Ok(client.clone());
        }

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
    mut params: TerminalCreateParams,
) -> Result<TerminalInfo, IpcError> {
    // Prefer the OS account's configured login shell (getpwuid) by default so
    // shells work consistently even when `nix develop` overrides $SHELL with
    // stdenv's minimal non-interactive bash (no readline).
    if params.argv.is_none()
        && params.line.is_none()
        && let Some(shell) = resolve_os_default_shell()
    {
        params.argv = Some(vec![shell]);
    }

    // Supply a default when LANG is missing, preserving the caller's locale overrides.
    let env_map = params.env.get_or_insert_with(Default::default);
    // The daemon's runtime path isolates holder startup from Linux's user manager.
    // Programs need the user's compositor/session sockets.
    if cfg!(target_os = "linux") {
        for key in ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"] {
            if let Ok(value) = std::env::var(key) {
                env_map.entry(key.into()).or_insert(value);
            }
        }
    }
    if !env_map.contains_key("LANG")
        && std::env::var("LANG")
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
    {
        let fallback_lang = std::env::var("LC_CTYPE")
            .ok()
            .filter(|v| v.to_uppercase().contains("UTF-8") || v.to_uppercase().contains("UTF8"))
            .unwrap_or_else(|| {
                if cfg!(target_os = "linux") {
                    "C.UTF-8"
                } else {
                    "en_US.UTF-8"
                }
                .to_owned()
            });
        env_map.insert("LANG".to_string(), fallback_lang);
    }

    let client = state.ensure_client().await?;
    client.terminal_create(params).await
}

#[cfg(unix)]
fn resolve_os_default_shell() -> Option<String> {
    let mut size = 1024;
    let pw_shell = loop {
        let mut buffer = vec![0; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();

        // The reentrant lookup stores the record and strings in our local buffers.
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if status != 0 || result.is_null() {
            break None;
        }
        let shell = unsafe { (*result).pw_shell };
        if shell.is_null() {
            break None;
        }
        break unsafe { std::ffi::CStr::from_ptr(shell) }
            .to_str()
            .ok()
            .map(str::to_owned);
    };

    if let Some(shell) = pw_shell
        && std::path::Path::new(&shell).is_file()
    {
        return Some(shell);
    }

    if let Ok(env_shell) = std::env::var("SHELL")
        && !env_shell.is_empty()
        && std::path::Path::new(&env_shell).is_file()
    {
        return Some(env_shell);
    }

    None
}

#[cfg(not(unix))]
fn resolve_os_default_shell() -> Option<String> {
    None
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

    let attach_id = result.attach_id;
    tokio::spawn(async move {
        while let Some(frame) = handle.rx.recv().await {
            let ipc_bytes = frame.into_ipc_bytes();
            if on_output.send(Response::new(ipc_bytes)).is_err() {
                let _ = client.terminal_detach(attach_id).await;
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

    let automatic = match request.headers().get("x-uiterm-input-kind") {
        None => false,
        Some(value) if value == "user" => false,
        Some(value) if value == "automatic" => true,
        _ => return Err(IpcError::new("invalid_args", "unknown input kind")),
    };

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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDelivery {
    subscription_id: u32,
    events: Vec<CoreEvent>,
    bytes: u32,
}

#[tauri::command]
pub async fn events_ack(
    state: State<'_, Arc<AppState>>,
    subscription_id: u32,
    bytes: u32,
) -> Result<(), IpcError> {
    state
        .ensure_client()
        .await?
        .events_ack(subscription_id, bytes)
        .await
}

#[tauri::command]
pub async fn events_subscribe(
    state: State<'_, Arc<AppState>>,
    on_event: Channel<EventDelivery>,
) -> Result<EventsSubscribeResult, IpcError> {
    let client = state.ensure_client().await?;
    let (result, mut rx) = client.events_subscribe().await?;

    let subscription_id = result.subscription_id;
    tokio::spawn(async move {
        while let Some(batch) = rx.recv().await {
            if on_event
                .send(EventDelivery {
                    subscription_id,
                    events: batch.events,
                    bytes: batch.bytes,
                })
                .is_err()
            {
                let _ = client.events_unsubscribe(subscription_id).await;
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
pub async fn config_eval_poc(mut params: ConfigEvalParams) -> Result<ConfigEvalResult, IpcError> {
    let sample = config_runtime::resolve_worker_script().with_file_name("sample.config.ts");
    if std::path::Path::new(&params.config_path)
        .canonicalize()
        .ok()
        .as_ref()
        != Some(&sample)
    {
        return Err(IpcError::new(
            "untrusted_config",
            "only the PoC sample config is available over IPC",
        ));
    }
    params.config_path = sample.to_string_lossy().into_owned();
    config_runtime::eval_config_poc(params).await
}
