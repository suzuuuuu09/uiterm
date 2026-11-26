//! Async Unix Domain Socket client for herdr-core Framed Mode.

use crate::herdr::protocol::{
    FRAME_AUTOMATIC_INPUT, FRAME_CLOSE, FRAME_CREDIT, FRAME_DATA, FRAME_RESIZE, Frame,
    INITIAL_CREDIT_BYTES, read_frame, write_frame,
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
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

/// Writer queue capacity for outbound frames to herdr-core.
const WRITER_QUEUE_CAPACITY: usize = 64;
const MAX_OUTBOUND_PAYLOAD: usize = 1024 * 1024;
const MAX_STREAMS: usize = 128;

#[derive(Debug)]
pub struct AttachHandle {
    pub result: TerminalAttachResult,
    pub rx: mpsc::Receiver<OutputStreamFrame>,
}

const IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
static NEXT_STREAM_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug)]
pub struct EventBatch {
    pub events: Vec<CoreEvent>,
    pub bytes: u32,
}

#[derive(Debug)]
enum Stream {
    Attach {
        tx: mpsc::Sender<OutputStreamFrame>,
        automatic: bool,
        outstanding: u32,
    },
    Events {
        tx: mpsc::Sender<EventBatch>,
        buffer: Vec<u8>,
        outstanding: u32,
    },
}

type RpcReply = Result<(Value, Option<u32>), IpcError>;

#[derive(Debug, Default)]
struct SharedState {
    pending_rpc: HashMap<u64, (oneshot::Sender<RpcReply>, Option<Stream>)>,
    streams: HashMap<u32, (u32, Stream)>,
    early_events: Vec<Frame>,
}

#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket_path: PathBuf,
    session: String,
    writer_tx: mpsc::Sender<Frame>,
    state: Arc<Mutex<SharedState>>,
    next_id: Arc<AtomicU64>,
    stop: watch::Sender<bool>,
    hello_features: Arc<HashSet<String>>,
    core_version: Arc<String>,
}

impl HerdrClient {
    /// Connect to herdr-core over Unix Domain Socket and upgrade to Framed Mode via `hello`.
    pub async fn connect(
        socket_path: impl AsRef<Path>,
        session: impl Into<String>,
    ) -> Result<Self, IpcError> {
        tokio::time::timeout(
            IO_TIMEOUT,
            Self::handshake(socket_path.as_ref().to_owned(), session.into()),
        )
        .await
        .map_err(|_| IpcError::new("core_timeout", "herdr-core hello timed out"))?
    }

    async fn handshake(socket_path: PathBuf, session: String) -> Result<Self, IpcError> {
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
        (&mut reader)
            .take(64 * 1024)
            .read_line(&mut resp_line)
            .await
            .map_err(|e| IpcError::new("io_error", format!("hello read failed: {e}")))?;
        if !resp_line.ends_with('\n') {
            return Err(IpcError::new(
                "protocol_error",
                "incomplete or oversized hello reply",
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

        let state = Arc::new(Mutex::new(SharedState::default()));
        let (writer_tx, mut writer_rx) = mpsc::channel::<Frame>(WRITER_QUEUE_CAPACITY);
        let (stop, mut stopped) = watch::channel(false);

        let socket_state = Arc::clone(&state);
        tokio::spawn(async move {
            let reason = tokio::select! {
                _ = stopped.changed() => "connection closed".to_owned(),
                reason = async {
                    while let Some(frame) = writer_rx.recv().await {
                        if let Err(err) = write_frame(&mut write_half, &frame).await {
                            return format!("socket write failed: {err}");
                        }
                    }
                    "writer closed".to_owned()
                } => reason,
                reason = async {
                    loop {
                        let frame = match read_frame(&mut reader).await {
                            Ok(frame) => frame,
                            Err(err) => return format!("socket read failed: {err}"),
                        };
                        let mut state = socket_state.lock().await;
                        if let Err(err) = dispatch_incoming_frame(&mut state, frame) {
                            return err.message;
                        }
                    }
                } => reason,
            };

            writer_rx.close();
            notify_disconnect(&socket_state, reason).await;
        });

        Ok(Self {
            socket_path,
            session,
            writer_tx,
            state,
            next_id: Arc::new(AtomicU64::new(2)),
            stop,
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
        self.rpc_inner(method, params, None)
            .await
            .map(|(value, _)| value)
    }

    pub fn is_connected(&self) -> bool {
        !*self.stop.borrow() && !self.writer_tx.is_closed()
    }

    async fn rpc_inner(&self, method: &str, params: Value, stream: Option<Stream>) -> RpcReply {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))
        .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;
        let frame = Frame::data(0, payload);
        validate_outbound(&frame)?;
        let (tx, rx) = oneshot::channel();

        let reply = tokio::time::timeout(IO_TIMEOUT, async {
            let permit = self.writer_tx.reserve().await.map_err(|_| disconnected())?;
            {
                let mut st = self.state.lock().await;
                if !self.is_connected() {
                    return Err(disconnected());
                }
                if st.pending_rpc.len() >= WRITER_QUEUE_CAPACITY {
                    return Err(IpcError::new("core_busy", "too many pending RPC requests"));
                }
                if stream.is_some()
                    && st.streams.len()
                        + st.pending_rpc
                            .values()
                            .filter(|(_, stream)| stream.is_some())
                            .count()
                        >= MAX_STREAMS
                {
                    return Err(IpcError::new("core_busy", "too many open streams"));
                }
                st.pending_rpc.insert(id, (tx, stream));
                permit.send(frame);
            }
            rx.await.map_err(|_| disconnected())?
        })
        .await;

        match reply {
            Ok(result) => result,
            Err(_) => {
                self.stop.send_replace(true);
                Err(IpcError::new("core_timeout", format!("{method} timed out")))
            }
        }
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
        if let (Some(cols), Some(rows)) = (params.cols, params.rows) {
            validate_size(cols, rows)?;
        }
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
        let address = val["address"].as_str().map(str::to_owned);
        match serde_json::from_value::<HerdrTerminalRecord>(val) {
            Ok(record) => Ok(record.into()),
            Err(err) => {
                if let Some(address) = address {
                    let _ = self.terminal_kill(&address).await;
                }
                Err(IpcError::new(
                    "protocol_error",
                    format!("invalid RunResult: {err}"),
                ))
            }
        }
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
        validate_size(params.cols, params.rows)?;
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

        let (tx, rx) = mpsc::channel(4097);
        let (val, id) = self
            .rpc_inner(
                "terminal.attach",
                Value::Object(obj),
                Some(Stream::Attach {
                    tx,
                    automatic: self.supports_automatic_input(),
                    outstanding: 0,
                }),
            )
            .await?;
        let attach_id = id.expect("successful stream reply registers a stream");
        let automatic_input = self.supports_automatic_input()
            && val
                .get("automatic_input")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let resize_notifications = val
            .get("resize_notifications")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let terminal = match serde_json::from_value::<HerdrTerminalRecord>(val["terminal"].clone())
        {
            Ok(terminal) => terminal,
            Err(err) => {
                let _ = self.terminal_detach(attach_id).await;
                return Err(IpcError::new(
                    "protocol_error",
                    format!("invalid attach terminal: {err}"),
                ));
            }
        };

        Ok(AttachHandle {
            result: TerminalAttachResult {
                attach_id,
                terminal: terminal.into(),
                automatic_input,
                resize_notifications,
                initial_credit_bytes: INITIAL_CREDIT_BYTES,
            },
            rx,
        })
    }

    pub async fn terminal_detach(&self, attach_id: u32) -> Result<(), IpcError> {
        self.send_stream_frame(Frame::close(attach_id, Vec::new()), false)
            .await
    }

    pub async fn terminal_write(
        &self,
        attach_id: u32,
        bytes: Vec<u8>,
        automatic: bool,
    ) -> Result<(), IpcError> {
        let frame = if automatic {
            Frame::automatic_input(attach_id, bytes)
        } else {
            Frame::data(attach_id, bytes)
        };
        self.send_stream_frame(frame, false).await
    }

    pub async fn terminal_resize(
        &self,
        attach_id: u32,
        cols: u16,
        rows: u16,
    ) -> Result<(), IpcError> {
        validate_size(cols, rows)?;
        self.send_stream_frame(Frame::resize(attach_id, cols, rows), false)
            .await
    }

    pub async fn terminal_focus(&self, attach_id: u32, focused: bool) -> Result<(), IpcError> {
        self.send_stream_frame(Frame::focus(attach_id, focused), false)
            .await
    }

    pub async fn terminal_facts(
        &self,
        attach_id: u32,
        display: DisplayFacts,
    ) -> Result<(), IpcError> {
        let json_bytes = serde_json::to_vec(&display)
            .map_err(|e| IpcError::new("serialize_error", e.to_string()))?;
        self.send_stream_frame(Frame::facts(attach_id, json_bytes), false)
            .await
    }

    pub async fn terminal_ack(&self, attach_id: u32, bytes: u32) -> Result<(), IpcError> {
        self.send_stream_frame(Frame::credit(attach_id, bytes), false)
            .await
    }

    pub async fn terminal_kill(&self, session_id: &str) -> Result<(), IpcError> {
        let _ = self
            .rpc("terminal.kill", json!({ "terminal": session_id }))
            .await?;
        Ok(())
    }

    pub async fn events_subscribe(
        &self,
    ) -> Result<(EventsSubscribeResult, mpsc::Receiver<EventBatch>), IpcError> {
        let (tx, rx) = mpsc::channel(257);
        let (val, id) = self
            .rpc_inner(
                "events.subscribe",
                json!({}),
                Some(Stream::Events {
                    tx,
                    buffer: Vec::new(),
                    outstanding: 0,
                }),
            )
            .await?;
        let subscription_id = id.expect("successful stream reply registers a stream");
        let seq = val.get("seq").and_then(Value::as_u64).unwrap_or(0);
        let terminals = val.get("terminals").cloned().unwrap_or_else(|| json!([]));
        let recs = match serde_json::from_value::<Vec<HerdrTerminalRecord>>(terminals) {
            Ok(recs) => recs,
            Err(err) => {
                let _ = self.events_unsubscribe(subscription_id).await;
                return Err(IpcError::new(
                    "protocol_error",
                    format!("invalid snapshot: {err}"),
                ));
            }
        };

        Ok((
            EventsSubscribeResult {
                subscription_id,
                seq,
                terminals: recs.into_iter().map(Into::into).collect(),
            },
            rx,
        ))
    }

    pub async fn events_unsubscribe(&self, subscription_id: u32) -> Result<(), IpcError> {
        self.send_stream_frame(Frame::close(subscription_id, Vec::new()), true)
            .await
    }

    pub async fn events_ack(&self, subscription_id: u32, bytes: u32) -> Result<(), IpcError> {
        self.send_stream_frame(Frame::credit(subscription_id, bytes), true)
            .await
    }

    async fn send_stream_frame(&self, mut frame: Frame, events: bool) -> Result<(), IpcError> {
        ensure_valid_attach_id(frame.channel)?;
        validate_outbound(&frame)?;
        let permit = tokio::time::timeout(IO_TIMEOUT, self.writer_tx.reserve())
            .await
            .map_err(|_| {
                self.stop.send_replace(true);
                IpcError::new("core_timeout", "herdr-core writer stalled")
            })?
            .map_err(|_| disconnected())?;
        let mut state = self.state.lock().await;
        if !self.is_connected() {
            return Err(disconnected());
        }
        let (&channel, (_, stream)) = state
            .streams
            .iter_mut()
            .find(|(_, (id, stream))| {
                *id == frame.channel && matches!(stream, Stream::Events { .. }) == events
            })
            .ok_or_else(|| IpcError::new("invalid_attach", "stream is no longer attached"))?;
        if frame.kind == FRAME_AUTOMATIC_INPUT
            && matches!(
                stream,
                Stream::Attach {
                    automatic: false,
                    ..
                }
            )
        {
            return Err(IpcError::new(
                "unsupported",
                "automatic input was not negotiated",
            ));
        }
        if frame.kind == FRAME_CREDIT {
            let outstanding = match stream {
                Stream::Attach { outstanding, .. } | Stream::Events { outstanding, .. } => {
                    outstanding
                }
            };
            let bytes = u32::from_le_bytes(frame.payload[..4].try_into().unwrap());
            *outstanding = outstanding.checked_sub(bytes).ok_or_else(|| {
                IpcError::new("invalid_args", "ACK exceeds unacknowledged output")
            })?;
        }
        if frame.kind == FRAME_CLOSE {
            state.streams.remove(&channel);
        }
        frame.channel = channel;
        permit.send(frame);
        Ok(())
    }
}

fn validate_outbound(frame: &Frame) -> Result<(), IpcError> {
    if frame.payload.len() > MAX_OUTBOUND_PAYLOAD {
        return Err(IpcError::new(
            "invalid_args",
            "outbound frame exceeds 1 MiB",
        ));
    }
    Ok(())
}

fn validate_size(cols: u16, rows: u16) -> Result<(), IpcError> {
    if !(1..=1000).contains(&cols) || !(1..=1000).contains(&rows) {
        return Err(IpcError::new(
            "invalid_args",
            "terminal dimensions must be between 1 and 1000",
        ));
    }
    Ok(())
}

fn disconnected() -> IpcError {
    IpcError::new("core_disconnected", "herdr-core connection closed")
}

fn ensure_valid_attach_id(attach_id: u32) -> Result<(), IpcError> {
    if attach_id == 0 {
        return Err(IpcError::new(
            "invalid_args",
            "attachId must be > 0 (channel 0 is reserved for JSON-RPC control)",
        ));
    }
    Ok(())
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

fn dispatch_incoming_frame(state: &mut SharedState, frame: Frame) -> Result<(), IpcError> {
    let protocol_error = |message: &str| IpcError::new("protocol_error", message);
    if frame.channel == 0 {
        if frame.kind != FRAME_DATA {
            return Err(protocol_error("unexpected control frame"));
        }
        let value: Value = serde_json::from_slice(&frame.payload)
            .map_err(|_| protocol_error("invalid control reply"))?;
        let id = value["id"]
            .as_u64()
            .ok_or_else(|| protocol_error("missing RPC id"))?;
        if let Some((tx, stream)) = state.pending_rpc.remove(&id) {
            let result = extract_rpc_result(value);
            let expects_stream = result.is_ok() && stream.is_some();
            let mut opened_event_channel = None;
            let reply = result.and_then(|result| {
                let id = if let Some(mut stream) = stream {
                    let channel = result["channel"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|&n| n != 0 && !state.streams.contains_key(&n))
                        .ok_or_else(|| protocol_error("invalid or duplicate stream channel"))?;
                    let stream_id = NEXT_STREAM_ID
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                        .map_err(|_| protocol_error("stream IDs exhausted"))?;
                    if let Stream::Attach { automatic, .. } = &mut stream {
                        *automatic &= result["automatic_input"].as_bool() == Some(true);
                    }
                    if matches!(stream, Stream::Events { .. }) {
                        opened_event_channel = Some(channel);
                    }
                    state.streams.insert(channel, (stream_id, stream));
                    Some(stream_id)
                } else {
                    None
                };
                Ok((result, id))
            });
            if let Some(channel) = opened_event_channel {
                for early in std::mem::take(&mut state.early_events) {
                    if early.channel == channel {
                        dispatch_incoming_frame(state, early)?;
                    } else {
                        state.early_events.push(early);
                    }
                }
            }
            if !state
                .pending_rpc
                .values()
                .any(|(_, stream)| matches!(stream, Some(Stream::Events { .. })))
            {
                state.early_events.clear();
            }
            let invalid_stream = expects_stream && reply.is_err();
            let cancelled_stream = tx.send(reply).is_err() && expects_stream;
            if invalid_stream || cancelled_stream {
                return Err(protocol_error("stream could not be registered"));
            }
        }
        return Ok(());
    }

    let Some((_, stream)) = state.streams.get_mut(&frame.channel) else {
        // v0.1.22 opens events unpaused, so a notification can precede its RPC reply.
        if frame.kind == FRAME_DATA
            && state
                .pending_rpc
                .values()
                .any(|(_, stream)| matches!(stream, Some(Stream::Events { .. })))
        {
            let buffered = state
                .early_events
                .iter()
                .map(|item| item.payload.len())
                .sum::<usize>();
            if state.early_events.len() >= 256
                || frame.payload.len() > INITIAL_CREDIT_BYTES as usize - buffered
            {
                return Err(protocol_error("early event buffer full"));
            }
            state.early_events.push(frame);
        }
        return Ok(());
    };

    if frame.kind == FRAME_CLOSE {
        match state.streams.remove(&frame.channel) {
            Some((_, Stream::Attach { tx, .. })) => {
                let _ = tx.try_send(OutputStreamFrame::Close(frame.payload));
            }
            Some((_, Stream::Events { tx, .. })) => {
                let _ = tx.try_send(EventBatch {
                    events: vec![CoreEvent::Disconnected {
                        reason: "event stream closed".into(),
                    }],
                    bytes: 0,
                });
            }
            None => {}
        }
        return Ok(());
    }

    match stream {
        Stream::Attach {
            tx, outstanding, ..
        } => {
            if tx.capacity() <= 1 && !tx.is_closed() {
                return Err(protocol_error("terminal frame queue full"));
            }
            let item = match frame.kind {
                FRAME_DATA if !frame.payload.is_empty() => {
                    let len = frame.payload.len() as u32;
                    if len > INITIAL_CREDIT_BYTES - *outstanding {
                        return Err(protocol_error("terminal exceeded its credit window"));
                    }
                    *outstanding += len;
                    OutputStreamFrame::Data(frame.payload)
                }
                FRAME_RESIZE => {
                    let (cols, rows) = frame
                        .parse_resize()
                        .ok_or_else(|| protocol_error("invalid resize payload"))?;
                    validate_size(cols, rows)?;
                    OutputStreamFrame::Resize { cols, rows }
                }
                _ => return Ok(()),
            };
            if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(item) {
                return Err(protocol_error("terminal frame queue full"));
            }
        }
        Stream::Events {
            tx,
            buffer,
            outstanding,
        } if frame.kind == FRAME_DATA && !frame.payload.is_empty() => {
            let bytes = frame.payload.len() as u32;
            if bytes > INITIAL_CREDIT_BYTES - *outstanding {
                return Err(protocol_error("events exceeded their credit window"));
            }
            if tx.capacity() <= 1 {
                return Err(protocol_error("event queue full"));
            }
            *outstanding += bytes;
            let mut events = Vec::new();
            if frame.payload.len() > INITIAL_CREDIT_BYTES as usize - buffer.len() {
                return Err(protocol_error("event buffer full"));
            }
            buffer.extend_from_slice(&frame.payload);
            let mut lines = buffer.split_inclusive(|&b| b == b'\n').peekable();
            let mut consumed = 0;
            while let Some(line) = lines.next() {
                let value = serde_json::from_slice::<Value>(line);
                if lines.peek().is_none() && !line.ends_with(b"\n") && value.is_err() {
                    break;
                }
                consumed += line.len();
                if let Ok(value) = value
                    && let Some(event) = parse_core_event(&value)
                {
                    events.push(event);
                }
            }
            buffer.drain(..consumed);
            let _ = tx.try_send(EventBatch { events, bytes });
        }
        _ => {}
    }
    Ok(())
}

async fn notify_disconnect(state: &Arc<Mutex<SharedState>>, reason: String) {
    let (pending, streams) = {
        let mut st = state.lock().await;
        (
            std::mem::take(&mut st.pending_rpc),
            std::mem::take(&mut st.streams),
        )
    };
    for (_, (tx, _)) in pending {
        let _ = tx.send(Err(IpcError::new("core_disconnected", reason.clone())));
    }
    for (_, (_, stream)) in streams {
        // Delivery waits only after the socket is closed, independently for each stream.
        let reason = reason.clone();
        tokio::spawn(async move {
            match stream {
                Stream::Attach { tx, .. } => {
                    let payload = json!({ "reason": "disconnected" }).to_string().into_bytes();
                    let _ = tx.send(OutputStreamFrame::Close(payload)).await;
                }
                Stream::Events { tx, .. } => {
                    let _ = tx
                        .send(EventBatch {
                            events: vec![CoreEvent::Disconnected { reason }],
                            bytes: 0,
                        })
                        .await;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn control_reply(id: u64, result: Value) -> Frame {
        Frame::data(
            0,
            serde_json::to_vec(&json!({"id": id, "result": result})).unwrap(),
        )
    }

    #[test]
    fn attach_routes_first_paint_and_close_before_caller_reads_reply() {
        let mut state = SharedState::default();
        let (reply, mut result) = oneshot::channel();
        let (tx, mut rx) = mpsc::channel(4);
        state.pending_rpc.insert(
            8,
            (
                reply,
                Some(Stream::Attach {
                    tx,
                    automatic: true,
                    outstanding: 0,
                }),
            ),
        );
        dispatch_incoming_frame(&mut state, control_reply(8, json!({"channel": 7}))).unwrap();
        dispatch_incoming_frame(&mut state, Frame::data(7, b"first".to_vec())).unwrap();
        dispatch_incoming_frame(&mut state, Frame::resize(7, 100, 30)).unwrap();
        dispatch_incoming_frame(&mut state, Frame::close(7, b"exited".to_vec())).unwrap();

        assert!(result.try_recv().unwrap().unwrap().1.is_some());
        assert!(
            matches!(rx.try_recv().ok(), Some(OutputStreamFrame::Data(bytes)) if bytes == b"first")
        );
        assert!(matches!(
            rx.try_recv().ok(),
            Some(OutputStreamFrame::Resize {
                cols: 100,
                rows: 30
            })
        ));
        assert!(
            matches!(rx.try_recv().ok(), Some(OutputStreamFrame::Close(bytes)) if bytes == b"exited")
        );
        assert!(rx.try_recv().ok().is_none());
    }

    #[test]
    fn events_replay_pre_reply_fragments_in_wire_order_with_credit_bytes() {
        let mut state = SharedState::default();
        let (reply, mut result) = oneshot::channel();
        let (tx, mut rx) = mpsc::channel(8);
        state.pending_rpc.insert(
            8,
            (
                reply,
                Some(Stream::Events {
                    tx,
                    buffer: Vec::new(),
                    outstanding: 0,
                }),
            ),
        );
        let first = b"{\"method\":\"heartbeat\",\"params\":{\"seq\":1";
        let second = b"}}\n{\"method\":\"heartbeat\",\"params\":{\"seq\":2}}\n";
        dispatch_incoming_frame(&mut state, Frame::data(7, first.to_vec())).unwrap();
        dispatch_incoming_frame(&mut state, control_reply(8, json!({"channel": 7}))).unwrap();
        dispatch_incoming_frame(&mut state, Frame::data(7, second.to_vec())).unwrap();
        assert!(result.try_recv().unwrap().unwrap().1.is_some());

        let first_batch = rx.try_recv().ok().unwrap();
        assert!(first_batch.events.is_empty());
        assert_eq!(first_batch.bytes as usize, first.len());
        let second_batch = rx.try_recv().ok().unwrap();
        assert_eq!(second_batch.bytes as usize, second.len());
        assert!(matches!(
            second_batch.events.as_slice(),
            [
                CoreEvent::Heartbeat { seq: 1 },
                CoreEvent::Heartbeat { seq: 2 }
            ]
        ));
    }

    #[tokio::test]
    async fn stale_or_oversized_credit_does_not_reach_the_wire() {
        let (writer_tx, mut writer_rx) = mpsc::channel(8);
        let (stop, _stopped) = watch::channel(false);
        let (tx, _rx) = mpsc::channel(4);
        let mut state = SharedState::default();
        state.streams.insert(
            7,
            (
                100,
                Stream::Attach {
                    tx,
                    automatic: false,
                    outstanding: 9,
                },
            ),
        );
        let client = HerdrClient {
            socket_path: PathBuf::new(),
            session: "test".into(),
            writer_tx,
            state: Arc::new(Mutex::new(state)),
            next_id: Arc::new(AtomicU64::new(1)),
            stop,
            hello_features: Arc::new(HashSet::new()),
            core_version: Arc::new("test".into()),
        };

        assert_eq!(
            client.terminal_ack(7, 1).await.unwrap_err().code,
            "invalid_attach"
        );
        assert_eq!(
            client.terminal_ack(100, 10).await.unwrap_err().code,
            "invalid_args"
        );
        assert_eq!(
            client
                .terminal_write(100, b"reply".to_vec(), true)
                .await
                .unwrap_err()
                .code,
            "unsupported"
        );
        assert!(writer_rx.try_recv().is_err());

        client.terminal_ack(100, 9).await.unwrap();
        assert_eq!(writer_rx.try_recv().unwrap(), Frame::credit(7, 9));
        client.terminal_detach(100).await.unwrap();
        assert_eq!(writer_rx.try_recv().unwrap(), Frame::close(7, vec![]));
        assert_eq!(
            client
                .terminal_write(100, b"key".to_vec(), false)
                .await
                .unwrap_err()
                .code,
            "invalid_attach"
        );
    }

    #[test]
    fn invalid_reply_channels_disconnect_without_reusing_a_channel() {
        for channel in [0, u32::MAX as u64 + 8, 7] {
            let mut state = SharedState::default();
            let (reply, mut result) = oneshot::channel();
            let (tx, _rx) = mpsc::channel(4);
            state.pending_rpc.insert(
                8,
                (
                    reply,
                    Some(Stream::Attach {
                        tx,
                        automatic: true,
                        outstanding: 0,
                    }),
                ),
            );
            let (existing, _existing_rx) = mpsc::channel(4);
            state.streams.insert(
                7,
                (
                    111,
                    Stream::Attach {
                        tx: existing,
                        automatic: false,
                        outstanding: 0,
                    },
                ),
            );
            let error =
                dispatch_incoming_frame(&mut state, control_reply(8, json!({"channel": channel})))
                    .unwrap_err();
            assert_eq!(error.code, "protocol_error");
            assert_eq!(
                result.try_recv().unwrap().unwrap_err().code,
                "protocol_error"
            );
            assert_eq!(state.streams.len(), 1);
            assert_eq!(state.streams[&7].0, 111);
        }
    }
}
