//! Phase 0 PoC: TypeScript config runtime over external Node.js + JSON-RPC over stdio.
//!
//! Node.js 24 LTS strips TypeScript types natively (no bundler/tsx required).
//! Node.js is NOT bundled into the Tauri app; it is invoked from the user's PATH.

use crate::herdr::types::IpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigEvalParams {
    pub config_path: String,
    #[cfg_attr(test, ts(optional = nullable))]
    pub action_name: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    #[cfg_attr(test, ts(optional = nullable))]
    pub action_args: Option<Value>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub event_name: Option<String>,
    #[cfg_attr(test, ts(type = "unknown"))]
    #[cfg_attr(test, ts(optional = nullable))]
    pub event_payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigEvalResult {
    #[cfg_attr(test, ts(type = "unknown"))]
    pub config: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub action_result: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub hook_result: Value,
    pub startup_ms: f64,
    pub rpc_ms: f64,
    pub worker_rss_bytes: Option<u64>,
}

pub struct ConfigWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl ConfigWorker {
    pub async fn spawn(worker_script: &Path) -> Result<(Self, f64), IpcError> {
        let start = Instant::now();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let node = std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("node"))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                IpcError::with_hint(
                    "node_spawn_failed",
                    "Node.js was not found in an absolute PATH entry",
                    Some("Ensure Node.js 24 LTS is available in PATH".into()),
                )
            })?;
        let mut child = Command::new(node)
            .arg(worker_script)
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                IpcError::with_hint(
                    "node_spawn_failed",
                    format!("failed to spawn external node: {e}"),
                    Some("Ensure Node.js 24 LTS is available in PATH".into()),
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| IpcError::new("io_error", "missing worker stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| IpcError::new("io_error", "missing worker stdout"))?;

        let mut worker = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        };

        // Wait for ping/ready handshake so startup_ms reflects true cold-start cost.
        let _ = worker.call("ping", json!({})).await?;
        let startup_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok((worker, startup_ms))
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, IpcError> {
        match tokio::time::timeout(Duration::from_secs(10), self.exchange(method, params)).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(err)) => {
                if err.code != "config_error" {
                    let _ = self.child.start_kill();
                }
                Err(err)
            }
            Err(_) => {
                let _ = self.child.start_kill();
                Err(IpcError::new(
                    "config_timeout",
                    format!("Node worker did not answer {method}"),
                ))
            }
        }
    }

    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value, IpcError> {
        const MAX_WORKER_REPLY_BYTES: u64 = 1024 * 1024;
        let id = self.next_id;
        self.next_id += 1;

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&req)
            .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| IpcError::new("io_error", format!("worker write failed: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| IpcError::new("io_error", format!("worker flush failed: {e}")))?;

        let mut resp_line = String::new();
        let n = (&mut self.stdout)
            .take(MAX_WORKER_REPLY_BYTES)
            .read_line(&mut resp_line)
            .await
            .map_err(|e| IpcError::new("io_error", format!("worker read failed: {e}")))?;
        if n == 0 {
            return Err(IpcError::new(
                "worker_exited",
                "TypeScript config worker exited unexpectedly",
            ));
        }
        if !resp_line.ends_with('\n') {
            let detail = if n as u64 >= MAX_WORKER_REPLY_BYTES {
                format!("worker reply exceeded {MAX_WORKER_REPLY_BYTES} byte limit")
            } else {
                "incomplete worker reply".to_string()
            };
            return Err(IpcError::new("protocol_error", detail));
        }

        let resp: Value = serde_json::from_str(&resp_line)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid worker JSON: {e}")))?;
        if resp["id"].as_u64() != Some(id) {
            return Err(IpcError::new(
                "protocol_error",
                "mismatched worker reply id",
            ));
        }
        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("config worker error");
            return Err(IpcError::new("config_error", msg));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn shutdown(mut self) -> Result<(), IpcError> {
        let _ = self.call("shutdown", json!({})).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), self.child.wait()).await;
        let _ = self.child.kill().await;
        Ok(())
    }
}

pub fn resolve_worker_script() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap_or(&manifest_dir)
        .join("poc/config-runtime/worker.mjs")
}

pub async fn eval_config_poc(params: ConfigEvalParams) -> Result<ConfigEvalResult, IpcError> {
    let worker_script = resolve_worker_script();
    let (mut worker, startup_ms) = ConfigWorker::spawn(&worker_script).await?;

    let rpc_start = Instant::now();
    let load_res = worker
        .call("config.load", json!({ "path": params.config_path }))
        .await?;

    let action_result = if let Some(name) = params.action_name {
        worker
            .call(
                "action.invoke",
                json!({
                    "name": name,
                    "args": params.action_args.unwrap_or(Value::Null),
                }),
            )
            .await?
    } else {
        Value::Null
    };

    let hook_result = if let Some(event) = params.event_name {
        worker
            .call(
                "hook.emit",
                json!({
                    "event": event,
                    "payload": params.event_payload.unwrap_or(Value::Null),
                }),
            )
            .await?
    } else {
        Value::Null
    };

    let metrics = worker.call("runtime.metrics", json!({})).await?;
    let rpc_ms = rpc_start.elapsed().as_secs_f64() * 1000.0;
    let worker_rss_bytes = metrics.get("rssBytes").and_then(Value::as_u64);

    worker.shutdown().await?;

    Ok(ConfigEvalResult {
        config: load_res,
        action_result,
        hook_result,
        startup_ms,
        rpc_ms,
        worker_rss_bytes,
    })
}
