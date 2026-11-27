//! Tauri 2 IPC Commands & Channels implementing docs/poc/interface.md.

use crate::app_state::AppState;
use crate::config_runtime::{self, ConfigEvalParams, ConfigEvalResult};
use crate::herdr::{
    CoreEvent, CoreStatus, DisplayFacts, EventsSubscribeResult, IpcError, TerminalAttachParams,
    TerminalAttachResult, TerminalCreateParams, TerminalInfo,
};
use crate::os::resolve_os_default_shell;
use std::sync::Arc;
use tauri::State;
use tauri::ipc::{Channel, InvokeBody, Request, Response};

#[tauri::command]
pub(crate) async fn core_status(state: State<'_, Arc<AppState>>) -> Result<CoreStatus, IpcError> {
    let client = state.ensure_client().await?;
    client.server_status().await
}

#[tauri::command]
pub(crate) async fn core_reconnect(
    state: State<'_, Arc<AppState>>,
) -> Result<CoreStatus, IpcError> {
    let client = state.ensure_client().await?;
    client.server_status().await
}

#[tauri::command]
pub(crate) async fn terminal_create(
    state: State<'_, Arc<AppState>>,
    mut params: TerminalCreateParams,
) -> Result<TerminalInfo, IpcError> {
    params.validate()?;
    apply_default_shell_and_env(&mut params);
    apply_pty_iutf8_workaround(&mut params);
    let client = state.ensure_client().await?;
    client.terminal_create(params).await
}

fn supports_login_flag(shell: &str) -> bool {
    matches!(
        std::path::Path::new(shell)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("fish" | "zsh" | "bash" | "sh" | "dash" | "ksh" | "mksh")
    )
}

fn resolve_xdg_dir(env_key: &str, home_subpath: &str) -> Option<String> {
    if let Ok(value) = std::env::var(env_key)
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        std::path::Path::new(&home)
            .join(home_subpath)
            .to_string_lossy()
            .into_owned(),
    )
}

fn apply_default_shell_and_env(params: &mut TerminalCreateParams) {
    let env_map = params.env.get_or_insert_with(Default::default);

    // Prefer the OS account's configured login shell (getpwuid) by default so
    // shells work consistently even when `nix develop` overrides $SHELL with
    // stdenv's minimal non-interactive bash (no readline).
    if params.argv.is_none()
        && params.line.is_none()
        && let Some(shell) = resolve_os_default_shell()
    {
        env_map.entry("SHELL".into()).or_insert_with(|| shell.clone());
        params.argv = if supports_login_flag(&shell) {
            Some(vec![shell, "-l".to_string()])
        } else {
            Some(vec![shell])
        };
    }

    // HerdrDaemon isolates herdr-core by setting XDG_CONFIG_HOME / XDG_STATE_HOME to a
    // private /tmp/uiterm.* directory. Restore the user's XDG directories for child shells
    // so fish (~/.config/fish), nvim, starship, etc. load the user's actual configuration.
    if let Some(xdg_config) = resolve_xdg_dir("XDG_CONFIG_HOME", ".config") {
        env_map.entry("XDG_CONFIG_HOME".into()).or_insert(xdg_config);
    }
    if let Some(xdg_state) = resolve_xdg_dir("XDG_STATE_HOME", ".local/state") {
        env_map.entry("XDG_STATE_HOME".into()).or_insert(xdg_state);
    }

    // The daemon's runtime path isolates holder startup from Linux's user manager.
    // Programs need the user's compositor/session sockets.
    if cfg!(target_os = "linux") {
        for key in ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"] {
            if let Ok(value) = std::env::var(key) {
                env_map.entry(key.into()).or_insert(value);
            }
        }
    }

    // Supply a default when LANG is missing, preserving the caller's locale overrides.
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
}

/// Temporary workaround for `herdr-core` v0.1.22 opening new Unix PTYs without `IUTF8`.
///
/// Wraps the launched command with `uiterm --uiterm-pty-init -- <argv...>`, which enables
/// `IUTF8` on stdin via `tcsetattr` and immediately `exec`s the target process in-place.
/// Remove once upstream `herdr-core` sets `IUTF8` natively on new PTYs.
fn apply_pty_iutf8_workaround(params: &mut TerminalCreateParams) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("uiterm: skipping PTY IUTF8 workaround: current_exe unavailable");
            return;
        };
        let Some(exe) = exe.to_str() else {
            eprintln!("uiterm: skipping PTY IUTF8 workaround: current_exe path is not valid UTF-8");
            return;
        };
        wrap_params_with_pty_iutf8_init(params, exe);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = params;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn wrap_params_with_pty_iutf8_init(params: &mut TerminalCreateParams, exe: &str) {
    use crate::os::PTY_IUTF8_INIT_FLAG;

    if let Some(argv) = params.argv.as_mut() {
        if argv.is_empty()
            || (argv.len() >= 3
                && argv[0] == exe
                && argv[1] == PTY_IUTF8_INIT_FLAG
                && argv[2] == "--")
        {
            return;
        }
        let mut wrapped = Vec::with_capacity(argv.len() + 3);
        wrapped.push(exe.to_owned());
        wrapped.push(PTY_IUTF8_INIT_FLAG.to_owned());
        wrapped.push("--".to_owned());
        wrapped.append(argv);
        *argv = wrapped;
    } else if let Some(line) = params.line.take() {
        params.argv = Some(vec![
            exe.to_owned(),
            PTY_IUTF8_INIT_FLAG.to_owned(),
            "--".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            line,
        ]);
    }
}

#[tauri::command]
pub(crate) async fn terminal_list(
    state: State<'_, Arc<AppState>>,
    all: bool,
) -> Result<Vec<TerminalInfo>, IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_list(all).await
}

#[tauri::command]
pub(crate) async fn terminal_attach(
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
pub(crate) async fn terminal_detach(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_detach(attach_id).await
}

#[tauri::command]
pub(crate) async fn terminal_write(
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
        InvokeBody::Raw(raw) => raw.as_slice(),
        InvokeBody::Json(serde_json::Value::String(text)) => text.as_bytes(),
        _ => {
            return Err(IpcError::new(
                "invalid_args",
                "terminal_write expects raw binary data",
            ));
        }
    };
    if bytes.len() > crate::herdr::client::MAX_OUTBOUND_PAYLOAD || attach_id == 0 {
        return Err(IpcError::new(
            "invalid_args",
            "terminal_write requires a nonzero attach ID and at most 1 MiB",
        ));
    }

    let client = state.ensure_client().await?;
    client
        .terminal_write(attach_id, bytes.to_vec(), automatic)
        .await
}

#[tauri::command]
pub(crate) async fn terminal_resize(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_resize(attach_id, cols, rows).await
}

#[tauri::command]
pub(crate) async fn terminal_focus(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    focused: bool,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_focus(attach_id, focused).await
}

#[tauri::command]
pub(crate) async fn terminal_facts(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    display: DisplayFacts,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_facts(attach_id, display).await
}

#[tauri::command]
pub(crate) async fn terminal_ack(
    state: State<'_, Arc<AppState>>,
    attach_id: u32,
    bytes: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_ack(attach_id, bytes).await
}

#[tauri::command]
pub(crate) async fn terminal_kill(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.terminal_kill(&session_id).await
}

#[derive(serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EventDelivery {
    subscription_id: u32,
    events: Vec<CoreEvent>,
    bytes: u32,
}

#[tauri::command]
pub(crate) async fn events_ack(
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
pub(crate) async fn events_subscribe(
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
pub(crate) async fn events_unsubscribe(
    state: State<'_, Arc<AppState>>,
    subscription_id: u32,
) -> Result<(), IpcError> {
    let client = state.ensure_client().await?;
    client.events_unsubscribe(subscription_id).await
}

#[tauri::command]
pub(crate) async fn config_eval_poc(
    mut params: ConfigEvalParams,
) -> Result<ConfigEvalResult, IpcError> {
    if !cfg!(debug_assertions)
        || std::env::var_os("UITERM_ENABLE_CONFIG_POC").as_deref()
            != Some(std::ffi::OsStr::new("1"))
    {
        return Err(IpcError::new(
            "untrusted_config",
            "configuration evaluation is disabled",
        ));
    }

    let sample = config_runtime::resolve_worker_script()
        .with_file_name("sample.config.ts")
        .canonicalize()
        .map_err(|err| {
            IpcError::new(
                "io_error",
                format!("failed to resolve sample config: {err}"),
            )
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_sets_login_flag_and_restores_xdg_and_shell_env() {
        let mut params = TerminalCreateParams::default();
        apply_default_shell_and_env(&mut params);

        let env = params.env.as_ref().expect("env map populated");
        if let Some(shell) = resolve_os_default_shell() {
            assert_eq!(env.get("SHELL").map(String::as_str), Some(shell.as_str()));
            let argv = params.argv.as_ref().expect("argv populated");
            assert_eq!(argv[0], shell);
            if supports_login_flag(&shell) {
                assert_eq!(argv.get(1).map(String::as_str), Some("-l"));
            }
        }
        assert!(
            env.get("XDG_CONFIG_HOME")
                .is_some_and(|v| !v.starts_with("/tmp/uiterm.")),
            "XDG_CONFIG_HOME must point to user's config dir, got {:?}",
            env.get("XDG_CONFIG_HOME")
        );
        assert!(
            env.get("XDG_STATE_HOME")
                .is_some_and(|v| !v.starts_with("/tmp/uiterm.")),
            "XDG_STATE_HOME must point to user's state dir, got {:?}",
            env.get("XDG_STATE_HOME")
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pty_iutf8_workaround_wraps_argv_and_line_idempotently() {
        let mut argv_params = TerminalCreateParams {
            argv: Some(vec!["/bin/fish".into(), "-l".into()]),
            ..Default::default()
        };
        wrap_params_with_pty_iutf8_init(&mut argv_params, "/app/uiterm");
        assert_eq!(
            argv_params.argv.as_deref(),
            Some(
                &[
                    "/app/uiterm".to_string(),
                    crate::os::PTY_IUTF8_INIT_FLAG.to_string(),
                    "--".to_string(),
                    "/bin/fish".to_string(),
                    "-l".to_string(),
                ][..]
            )
        );
        // Calling a second time must be a no-op.
        wrap_params_with_pty_iutf8_init(&mut argv_params, "/app/uiterm");
        assert_eq!(argv_params.argv.as_ref().unwrap().len(), 5);

        // An unrelated command that happens to pass the same flag must still be wrapped.
        let mut other_cmd = TerminalCreateParams {
            argv: Some(vec![
                "/usr/bin/env".into(),
                crate::os::PTY_IUTF8_INIT_FLAG.into(),
                "--".into(),
                "echo".into(),
            ]),
            ..Default::default()
        };
        wrap_params_with_pty_iutf8_init(&mut other_cmd, "/app/uiterm");
        assert_eq!(other_cmd.argv.as_ref().unwrap()[0], "/app/uiterm");
        assert_eq!(other_cmd.argv.as_ref().unwrap().len(), 7);

        let mut line_params = TerminalCreateParams {
            line: Some("echo hi".into()),
            ..Default::default()
        };
        wrap_params_with_pty_iutf8_init(&mut line_params, "/app/uiterm");
        assert!(line_params.line.is_none());
        assert_eq!(
            line_params.argv.as_deref(),
            Some(
                &[
                    "/app/uiterm".to_string(),
                    crate::os::PTY_IUTF8_INIT_FLAG.to_string(),
                    "--".to_string(),
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo hi".to_string(),
                ][..]
            )
        );
    }
}
