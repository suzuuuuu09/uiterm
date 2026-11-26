//! Async Unix Domain Socket client for herdr-core Framed Mode.

use crate::herdr::protocol::{
    FRAME_AUTOMATIC_INPUT, FRAME_CLOSE, FRAME_DATA, FRAME_RESIZE, Frame, INITIAL_CREDIT_BYTES,
    read_frame, write_frame,
};
use crate::herdr::types::{
    CoreEvent, CoreStatus, DisplayFacts, EventsSubscribeResult, ForegroundProcess,
    HerdrTerminalRecord, IpcError, OutputStreamFrame, TerminalAttachParams, TerminalAttachResult,
    TerminalCreateParams, TerminalInfo,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc, oneshot};

/// Writer queue capacity for outbound frames to herdr-core.
const WRITER_QUEUE_CAPACITY: usize = 256;

#[derive(Debug)]
pub struct AttachHandle {
    pub result: TerminalAttachResult,
    /// Per-stream receiver. Uses an unbounded channel in Rust so the single
    /// multiplexed Unix-socket reader NEVER head-of-line blocks channel 0 or
    /// other terminals; per-stream memory is strictly bounded by herdr-core's
    /// 256 KiB credit-based flow control (credit is only replenished via
    /// `terminal_ack` after the frontend consumes bytes).
    pub rx: mpsc::UnboundedReceiver<OutputStreamFrame>,
}

#[derive(Debug)]
struct SharedState {
    pending_rpc: HashMap<u64, oneshot::Sender<Result<Value, IpcError>>>,
    attach_streams: HashMap<u32, mpsc::UnboundedSender<OutputStreamFrame>>,
    attach_auto_input: HashMap<u32, bool>,
    event_streams: HashMap<u32, mpsc::UnboundedSender<CoreEvent>>,
    event_buffers: HashMap<u32, Vec<u8>>,
    /// Frames that arrived on a newly-opened channel in the tiny window between
    /// the reader delivering the channel-0 RPC reply and the caller registering
    /// the channel in `attach_streams` / `event_streams`.
    early_frames: HashMap<u32, Vec<Frame>>,
}

#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket_path: PathBuf,
    session: String,
    writer_tx: mpsc::Sender<Frame>,
    state: Arc<Mutex<SharedState>>,
    next_id: Arc<AtomicU64>,
    hello_features: Arc<HashSet<String>>,
    core_version: Arc<String>,
}

impl HerdrClient {
    /// Connect to herdr-core over Unix Domain Socket and upgrade to Framed Mode via `hello`.
    pub async fn connect(
        socket_path: impl AsRef<Path>,
        session: impl Into<String>,
    ) -> Result<Self, IpcError> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let session = session.into();
        let stream = UnixStream::connect(&socket_path).await.map_err(|e| {
            IpcError::new(
                "connect_failed",
                format!("failed to connect to {}: {e}", socket_path.display()),
            )
        })?;

        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // Plain-mode handshake: request framed mode + feature negotiation.
        let hello_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "hello",
            "params": {
                "framed": true,
                "generation": 1,
                "features": ["automatic_input", "resize_notifications"]
            }
        });
        let mut line = serde_json::to_string(&hello_req)
            .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;
        line.push('\n');
        write_half
            .write_all(line.as_bytes())
            .await
            .map_err(|e| IpcError::new("io_error", format!("hello write failed: {e}")))?;
        write_half
            .flush()
            .await
            .map_err(|e| IpcError::new("io_error", format!("hello flush failed: {e}")))?;

        let mut resp_line = String::new();
        let n = reader
            .read_line(&mut resp_line)
            .await
            .map_err(|e| IpcError::new("io_error", format!("hello read failed: {e}")))?;
        if n == 0 {
            return Err(IpcError::new(
                "core_disconnected",
                "herdr-core closed socket during hello handshake",
            ));
        }

        let resp_val: Value = serde_json::from_str(&resp_line)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid hello JSON: {e}")))?;
        let hello_res = extract_rpc_result(resp_val)?;

        if !hello_res
            .get("framed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(IpcError::new(
                "protocol_error",
                "herdr-core refused framed mode",
            ));
        }

        let features: HashSet<String> = hello_res
            .get("features")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let core_version = hello_res
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let state = Arc::new(Mutex::new(SharedState {
            pending_rpc: HashMap::new(),
            attach_streams: HashMap::new(),
            attach_auto_input: HashMap::new(),
            event_streams: HashMap::new(),
            event_buffers: HashMap::new(),
            early_frames: HashMap::new(),
        }));

        let (writer_tx, mut writer_rx) = mpsc::channel::<Frame>(WRITER_QUEUE_CAPACITY);

        // Writer task
        tokio::spawn(async move {
            while let Some(frame) = writer_rx.recv().await {
                if write_frame(&mut write_half, &frame).await.is_err() {
                    break;
                }
            }
        });

        // Reader & demultiplexer task
        let reader_state = Arc::clone(&state);
        tokio::spawn(async move {
            let disconnect_reason = loop {
                match read_frame(&mut reader).await {
                    Ok(frame) => {
                        dispatch_incoming_frame(&reader_state, frame).await;
                    }
                    Err(e) => {
                        break format!("socket closed: {e}");
                    }
                }
            };
            notify_disconnect(&reader_state, disconnect_reason).await;
        });

        Ok(Self {
            socket_path,
            session,
            writer_tx,
            state,
            next_id: Arc::new(AtomicU64::new(2)),
            hello_features: Arc::new(features),
            core_version: Arc::new(core_version),
        })
    }

    pub fn supports_automatic_input(&self) -> bool {
        self.hello_features.contains("automatic_input")
    }

    pub fn supports_resize_notifications(&self) -> bool {
        self.hello_features.contains("resize_notifications")
    }

    pub fn core_version(&self) -> &str {
        &self.core_version
    }

    /// Perform a JSON-RPC 2.0 call over channel 0 in Framed Mode.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, IpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.state.lock().await;
            st.pending_rpc.insert(id, tx);
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let payload = serde_json::to_vec(&req)
            .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;

        if self.writer_tx.send(Frame::data(0, payload)).await.is_err() {
            let mut st = self.state.lock().await;
            st.pending_rpc.remove(&id);
            return Err(IpcError::new(
                "core_disconnected",
                "herdr-core writer channel closed",
            ));
        }

        rx.await.map_err(|_| {
            IpcError::new(
                "core_disconnected",
                "herdr-core disconnected while waiting for RPC reply",
            )
        })?
    }

    pub async fn server_status(&self) -> Result<CoreStatus, IpcError> {
        let val = self.rpc("server.status", json!({})).await?;
        Ok(CoreStatus {
            version: val
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or(&self.core_version)
                .to_string(),
            pid: val.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            uptime_ms: val.get("uptime_ms").and_then(Value::as_u64).unwrap_or(0),
            live: val.get("live").and_then(Value::as_u64).unwrap_or(0),
            finished: val.get("finished").and_then(Value::as_u64).unwrap_or(0),
            session: self.session.clone(),
            socket_path: self.socket_path.to_string_lossy().to_string(),
            automatic_input_supported: self.supports_automatic_input(),
            resize_notifications_supported: self.supports_resize_notifications(),
        })
    }

    pub async fn terminal_create(
        &self,
        params: TerminalCreateParams,
    ) -> Result<TerminalInfo, IpcError> {
        let mut obj = serde_json::Map::new();
        if let Some(argv) = params.argv {
            obj.insert("argv".into(), json!(argv));
        }
        if let Some(line) = params.line {
            obj.insert("line".into(), json!(line));
        }
        if let Some(cwd) = params.cwd {
            obj.insert("cwd".into(), json!(cwd));
        }
        if let Some(env) = params.env {
            obj.insert("env".into(), json!(env));
        }
        if let Some(name) = params.name {
            obj.insert("name".into(), json!(name));
        }
        if let Some(labels) = params.labels {
            obj.insert("labels".into(), json!(labels));
        }
        if let (Some(cols), Some(rows)) = (params.cols, params.rows) {
            obj.insert("size".into(), json!([cols, rows]));
        }
        if let Some(display) = params.display {
            obj.insert("display".into(), json!(display));
        }
        if let Some(keep) = params.keep {
            obj.insert("keep".into(), json!(keep));
        }

        let val = self.rpc("terminal.run", Value::Object(obj)).await?;
        let rec: HerdrTerminalRecord = serde_json::from_value(val)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid RunResult: {e}")))?;
        Ok(rec.into())
    }

    pub async fn terminal_list(&self, all: bool) -> Result<Vec<TerminalInfo>, IpcError> {
        let val = self.rpc("terminal.list", json!({ "all": all })).await?;
        let arr = val
            .get("terminals")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));
        let recs: Vec<HerdrTerminalRecord> = serde_json::from_value(arr)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid terminal.list: {e}")))?;
        Ok(recs.into_iter().map(Into::into).collect())
    }

    pub async fn terminal_attach(
        &self,
        params: TerminalAttachParams,
    ) -> Result<AttachHandle, IpcError> {
        let mut obj = serde_json::Map::new();
        obj.insert("terminal".into(), json!(params.session_id));
        obj.insert("cols".into(), json!(params.cols));
        obj.insert("rows".into(), json!(params.rows));
        if let Some(history) = params.history {
            obj.insert("history".into(), json!(history));
        }
        if let Some(observe) = params.observe {
            obj.insert("observe".into(), json!(observe));
        }
        if let Some(rn) = params.resize_notifications {
            obj.insert("resize_notifications".into(), json!(rn));
        }
        if let Some(display) = params.display {
            obj.insert("display".into(), json!(display));
        }

        // Note: herdr-core guarantees the attach JSON-RPC reply arrives BEFORE any stream frames.
        // We register the stream channel immediately after receiving the reply.
        let val = self.rpc("terminal.attach", Value::Object(obj)).await?;

        let channel = val
            .get("channel")
            .and_then(Value::as_u64)
            .ok_or_else(|| IpcError::new("protocol_error", "attach reply missing channel"))?
            as u32;
        let automatic_input = self.supports_automatic_input()
            && val
                .get("automatic_input")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let resize_notifications = val
            .get("resize_notifications")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let terminal_val = val
            .get("terminal")
            .cloned()
            .ok_or_else(|| IpcError::new("protocol_error", "attach reply missing terminal"))?;
        let terminal: HerdrTerminalRecord = serde_json::from_value(terminal_val)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid attach terminal: {e}")))?;

        let (tx, rx) = mpsc::unbounded_channel();
        let early = {
            let mut st = self.state.lock().await;
            st.attach_streams.insert(channel, tx);
            st.attach_auto_input.insert(channel, automatic_input);
            st.early_frames.remove(&channel).unwrap_or_default()
        };
        for frame in early {
            dispatch_incoming_frame(&self.state, frame).await;
        }

        Ok(AttachHandle {
            result: TerminalAttachResult {
                attach_id: channel,
                terminal: terminal.into(),
                automatic_input,
                resize_notifications,
                initial_credit_bytes: INITIAL_CREDIT_BYTES,
            },
            rx,
        })
    }

    pub async fn terminal_detach(&self, attach_id: u32) -> Result<(), IpcError> {
        {
            let mut st = self.state.lock().await;
            st.attach_streams.remove(&attach_id);
            st.attach_auto_input.remove(&attach_id);
        }
        self.send_frame(Frame::close(attach_id, Vec::new())).await
    }

    /// Write bytes to an attached terminal.
    /// When `automatic` is true AND both `hello.features` and `terminal.attach`
    /// confirmed `automatic_input`, sends frame kind 6 (AUTOMATIC_INPUT);
    /// otherwise falls back to kind 0 (DATA) per herdr-core spec.
    pub async fn terminal_write(
        &self,
        attach_id: u32,
        bytes: Vec<u8>,
        automatic: bool,
    ) -> Result<(), IpcError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let use_auto = if automatic {
            let st = self.state.lock().await;
            st.attach_auto_input.get(&attach_id).copied().unwrap_or(false)
        } else {
            false
        };

        let frame = if use_auto {
            Frame::automatic_input(attach_id, bytes)
        } else {
            Frame::data(attach_id, bytes)
        };
        self.send_frame(frame).await
    }

    pub async fn terminal_resize(
        &self,
        attach_id: u32,
        cols: u16,
        rows: u16,
    ) -> Result<(), IpcError> {
        self.send_frame(Frame::resize(attach_id, cols, rows)).await
    }

    pub async fn terminal_focus(&self, attach_id: u32, focused: bool) -> Result<(), IpcError> {
        self.send_frame(Frame::focus(attach_id, focused)).await
    }

    pub async fn terminal_facts(
        &self,
        attach_id: u32,
        display: DisplayFacts,
    ) -> Result<(), IpcError> {
        let json_bytes = serde_json::to_vec(&display)
            .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;
        self.send_frame(Frame::facts(attach_id, json_bytes)).await
    }

    pub async fn terminal_ack(&self, attach_id: u32, bytes: u32) -> Result<(), IpcError> {
        if bytes == 0 {
            return Ok(());
        }
        self.send_frame(Frame::credit(attach_id, bytes)).await
    }

    pub async fn terminal_kill(&self, session_id: &str) -> Result<(), IpcError> {
        let _ = self
            .rpc("terminal.kill", json!({ "terminal": session_id }))
            .await?;
        Ok(())
    }

    pub async fn events_subscribe(
        &self,
    ) -> Result<(EventsSubscribeResult, mpsc::UnboundedReceiver<CoreEvent>), IpcError> {
        let val = self.rpc("events.subscribe", json!({})).await?;
        let channel = val
            .get("channel")
            .and_then(Value::as_u64)
            .ok_or_else(|| IpcError::new("protocol_error", "events.subscribe missing channel"))?
            as u32;
        let seq = val.get("seq").and_then(Value::as_u64).unwrap_or(0);
        let terminals_val = val
            .get("terminals")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));
        let recs: Vec<HerdrTerminalRecord> = serde_json::from_value(terminals_val)
            .map_err(|e| IpcError::new("protocol_error", format!("invalid snapshot: {e}")))?;

        let (tx, rx) = mpsc::unbounded_channel();
        let early = {
            let mut st = self.state.lock().await;
            st.event_streams.insert(channel, tx);
            st.early_frames.remove(&channel).unwrap_or_default()
        };
        for frame in early {
            dispatch_incoming_frame(&self.state, frame).await;
        }

        Ok((
            EventsSubscribeResult {
                subscription_id: channel,
                seq,
                terminals: recs.into_iter().map(Into::into).collect(),
            },
            rx,
        ))
    }

    pub async fn events_unsubscribe(&self, subscription_id: u32) -> Result<(), IpcError> {
        {
            let mut st = self.state.lock().await;
            st.event_streams.remove(&subscription_id);
            st.event_buffers.remove(&subscription_id);
        }
        self.send_frame(Frame::close(subscription_id, Vec::new()))
            .await
    }

    async fn send_frame(&self, frame: Frame) -> Result<(), IpcError> {
        self.writer_tx
            .send(frame)
            .await
            .map_err(|_| IpcError::new("core_disconnected", "herdr-core writer channel closed"))
    }
}

fn extract_rpc_result(resp: Value) -> Result<Value, IpcError> {
    if let Some(err) = resp.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown JSON-RPC error")
            .to_string();
        let data = err.get("data");
        let kind = data
            .and_then(|d| d.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("rpc_error")
            .to_string();
        let hint = data
            .and_then(|d| d.get("hint"))
            .and_then(Value::as_str)
            .map(String::from);
        return Err(IpcError::with_hint(kind, message, hint));
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

async fn dispatch_incoming_frame(state: &Arc<Mutex<SharedState>>, frame: Frame) {
    if frame.channel == 0 {
        if frame.kind == FRAME_DATA
            && let Ok(val) = serde_json::from_slice::<Value>(&frame.payload)
            && let Some(id) = val.get("id").and_then(Value::as_u64)
        {
            let tx = {
                let mut st = state.lock().await;
                st.pending_rpc.remove(&id)
            };
            if let Some(tx) = tx {
                let _ = tx.send(extract_rpc_result(val));
            }
        }
        return;
    }

    // Check if this channel is an attach stream.
    let attach_tx = {
        let st = state.lock().await;
        st.attach_streams.get(&frame.channel).cloned()
    };
    if let Some(tx) = attach_tx {
        let item = match frame.kind {
            FRAME_DATA => Some(OutputStreamFrame::Data(frame.payload)),
            FRAME_RESIZE => frame
                .parse_resize()
                .map(|(cols, rows)| OutputStreamFrame::Resize { cols, rows }),
            FRAME_CLOSE => {
                let mut st = state.lock().await;
                st.attach_streams.remove(&frame.channel);
                st.attach_auto_input.remove(&frame.channel);
                Some(OutputStreamFrame::Close(frame.payload))
            }
            FRAME_AUTOMATIC_INPUT => None,
            _ => None,
        };
        if let Some(item) = item {
            // Non-blocking send: avoids head-of-line blocking on the multiplexed
            // socket reader; per-stream buffering is bounded by herdr-core's
            // 256 KiB credit window.
            let _ = tx.send(item);
        }
        return;
    }

    // Otherwise check if this channel is an event subscription.
    // Event channels carry newline-delimited JSON-RPC notifications; a single
    // DATA frame may contain multiple notifications (e.g. `status` + `turn`)
    // or a partial line split across frames.
    let event_tx = {
        let st = state.lock().await;
        st.event_streams.get(&frame.channel).cloned()
    };
    if let Some(tx) = event_tx {
        match frame.kind {
            FRAME_DATA => {
                let lines: Vec<Vec<u8>> = {
                    let mut st = state.lock().await;
                    let buf = st.event_buffers.entry(frame.channel).or_default();
                    buf.extend_from_slice(&frame.payload);
                    let mut out = Vec::new();
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line = buf.drain(..=pos).collect::<Vec<u8>>();
                        out.push(line);
                    }
                    // Also handle a complete JSON object sent without trailing newline.
                    if out.is_empty() && !buf.is_empty() && serde_json::from_slice::<Value>(buf).is_ok() {
                        out.push(std::mem::take(buf));
                    }
                    out
                };
                for line in lines {
                    if let Ok(val) = serde_json::from_slice::<Value>(&line)
                        && let Some(ev) = parse_core_event(&val)
                    {
                        let _ = tx.send(ev);
                    }
                }
            }
            FRAME_CLOSE => {
                let mut st = state.lock().await;
                st.event_streams.remove(&frame.channel);
                st.event_buffers.remove(&frame.channel);
            }
            _ => {}
        }
        return;
    }

    // Channel not registered yet (race between channel-0 reply and caller registering
    // the stream). Buffer early frames so initial repaints/events are never dropped.
    let mut st = state.lock().await;
    let entry = st.early_frames.entry(frame.channel).or_default();
    if entry.len() < 64 {
        entry.push(frame);
    }
}

async fn notify_disconnect(state: &Arc<Mutex<SharedState>>, reason: String) {
    let (pending, attaches, events) = {
        let mut st = state.lock().await;
        st.attach_auto_input.clear();
        st.event_buffers.clear();
        st.early_frames.clear();
        (
            std::mem::take(&mut st.pending_rpc),
            std::mem::take(&mut st.attach_streams),
            std::mem::take(&mut st.event_streams),
        )
    };

    for (_, tx) in pending {
        let _ = tx.send(Err(IpcError::new("core_disconnected", reason.clone())));
    }

    let close_payload = json!({
        "reason": "disconnected",
        "exit_code": null,
        "signal": null
    })
    .to_string()
    .into_bytes();

    for (_, tx) in attaches {
        let _ = tx.send(OutputStreamFrame::Close(close_payload.clone()));
    }
    for (_, tx) in events {
        let _ = tx.send(CoreEvent::Disconnected {
            reason: reason.clone(),
        });
    }
}

fn parse_core_event(msg: &Value) -> Option<CoreEvent> {
    let method = msg.get("method")?.as_str()?;
    let params = msg.get("params")?;
    let seq = params.get("seq").and_then(Value::as_u64).unwrap_or(0);

    match method {
        "heartbeat" => Some(CoreEvent::Heartbeat { seq }),
        "missed" => {
            let terminals: Vec<HerdrTerminalRecord> = serde_json::from_value(
                params
                    .get("terminals")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(vec![])),
            )
            .ok()?;
            Some(CoreEvent::Missed {
                seq,
                terminals: terminals.into_iter().map(Into::into).collect(),
            })
        }
        "event" => {
            let ev_type = params.get("type")?.as_str()?;
            let terminal = params
                .get("terminal")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let data = params.get("data").cloned().unwrap_or(Value::Null);

            match ev_type {
                "created" => {
                    let rec: HerdrTerminalRecord = serde_json::from_value(data).ok()?;
                    Some(CoreEvent::Created {
                        seq,
                        terminal,
                        data: rec.into(),
                    })
                }
                "restarted" => {
                    let rec: HerdrTerminalRecord = serde_json::from_value(data).ok()?;
                    Some(CoreEvent::Restarted {
                        seq,
                        terminal,
                        data: rec.into(),
                    })
                }
                "lost" => Some(CoreEvent::Lost {
                    seq,
                    terminal,
                    data,
                }),
                "finished" => Some(CoreEvent::Finished {
                    seq,
                    terminal,
                    data,
                }),
                "removed" => Some(CoreEvent::Removed { seq, terminal }),
                "status" => Some(CoreEvent::Status {
                    seq,
                    terminal,
                    data,
                }),
                "turn" => Some(CoreEvent::Turn {
                    seq,
                    terminal,
                    data,
                }),
                "title" => Some(CoreEvent::Title {
                    seq,
                    terminal,
                    data,
                }),
                "size" => Some(CoreEvent::Size {
                    seq,
                    terminal,
                    data,
                }),
                "foreground" => {
                    let fg: Option<ForegroundProcess> = if data.is_null() {
                        None
                    } else {
                        serde_json::from_value(data).ok()
                    };
                    Some(CoreEvent::Foreground {
                        seq,
                        terminal,
                        data: fg,
                    })
                }
                "labels" => Some(CoreEvent::Labels {
                    seq,
                    terminal,
                    data,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}
