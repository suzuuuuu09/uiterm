//! Isolated `herdr-core` v0.1.22 Framed Mode client for the GPUI PoC.
//!
//! Strictly obeys AGENTS.md invariants:
//! - Always spawns a dedicated `herdr-core` in a private `0700` directory and socket.
//! - Never connects to the default Herdr socket or stops unrelated instances.
//! - Separates user input (`FRAME_DATA`, kind 0) from VT auto-responses
//!   (`FRAME_AUTOMATIC_INPUT`, kind 6) without ever falling back to `FRAME_DATA`.
//! - Never logs raw terminal input bytes or IME strings.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::os;

pub const CORE_VERSION: &str = "0.1.22";
pub const HEADER_LEN: usize = 9;
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;

pub const FRAME_DATA: u8 = 0;
pub const FRAME_CREDIT: u8 = 1;
pub const FRAME_CLOSE: u8 = 2;
pub const FRAME_RESIZE: u8 = 3;
pub const FRAME_FOCUS: u8 = 4;
pub const FRAME_AUTOMATIC_INPUT: u8 = 6;

static INSTANCE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn data(channel: u32, payload: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_DATA,
            payload,
        }
    }

    pub fn automatic_input(channel: u32, payload: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_AUTOMATIC_INPUT,
            payload,
        }
    }

    pub fn credit(channel: u32, bytes: u32) -> Self {
        Self {
            channel,
            kind: FRAME_CREDIT,
            payload: bytes.to_le_bytes().to_vec(),
        }
    }

    pub fn resize(channel: u32, cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_le_bytes());
        payload.extend_from_slice(&rows.to_le_bytes());
        Self {
            channel,
            kind: FRAME_RESIZE,
            payload,
        }
    }

    pub fn focus(channel: u32, focused: bool) -> Self {
        Self {
            channel,
            kind: FRAME_FOCUS,
            payload: vec![u8::from(focused)],
        }
    }

    pub fn encode_header(&self) -> io::Result<[u8; HEADER_LEN]> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame payload exceeds max",
            ));
        }
        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(&self.channel.to_le_bytes());
        header[4] = self.kind;
        header[5..9].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        Ok(header)
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    let header = frame.encode_header()?;
    writer.write_all(&header).await?;
    if !frame.payload.is_empty() {
        writer.write_all(&frame.payload).await?;
    }
    writer.flush().await
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Frame> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let kind = header[4];
    let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if len > MAX_PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incoming frame length exceeds max",
        ));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut payload).await?;
    }
    Ok(Frame {
        channel,
        kind,
        payload,
    })
}

pub fn resolve_herdr_core_bin() -> PathBuf {
    if let Ok(path) = std::env::var("HERDR_CORE_BIN")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("herdr-core");
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("herdr-core")
}

struct DirCleanupGuard(Option<PathBuf>);

impl DirCleanupGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for DirCleanupGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn create_private_dir(tag: &str) -> io::Result<PathBuf> {
    let seq = INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Keep Unix domain socket path well below macOS SUN_LEN (104 bytes).
    let dir = PathBuf::from(format!("/tmp/u-gp-{tag}-{pid}-{seq}"));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseInfo {
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub reason: String,
}

impl CloseInfo {
    pub fn from_payload(payload: &[u8]) -> Self {
        if payload.is_empty() {
            return Self {
                exit_code: None,
                signal: None,
                reason: "closed".to_string(),
            };
        }
        if let Ok(val) = serde_json::from_slice::<Value>(payload) {
            let exit_code = val.get("exit_code").and_then(Value::as_i64).map(|n| n as i32);
            let signal = match val.get("signal") {
                Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                Some(Value::Number(n)) => n.as_i64().map(format_signal_number),
                _ => None,
            };
            let reason = val
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("closed")
                .to_string();
            return Self {
                exit_code,
                signal,
                reason,
            };
        }
        Self {
            exit_code: None,
            signal: None,
            reason: String::from_utf8_lossy(payload).into_owned(),
        }
    }

    pub fn summary(&self) -> String {
        if let Some(sig) = &self.signal {
            format!("signal {sig} ({})", self.reason)
        } else if let Some(code) = self.exit_code {
            format!("exit {code} ({})", self.reason)
        } else {
            self.reason.clone()
        }
    }
}

fn format_signal_number(num: i64) -> String {
    match num {
        1 => "SIGHUP (1)".to_string(),
        2 => "SIGINT (2)".to_string(),
        3 => "SIGQUIT (3)".to_string(),
        9 => "SIGKILL (9)".to_string(),
        15 => "SIGTERM (15)".to_string(),
        other => format!("SIG({other})"),
    }
}

#[derive(Debug)]
pub enum SessionOutput {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Closed(CloseInfo),
}

#[derive(Debug, Clone)]
pub struct HerdrStartOptions {
    pub tag: String,
    pub argv: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub history_lines: usize,
    pub wrap_iutf8: bool,
}

impl Default for HerdrStartOptions {
    fn default() -> Self {
        let argv = os::default_shell_argv();
        let shell = argv.first().map(String::as_str);
        let env = os::build_child_shell_env(shell);
        Self {
            tag: "live".to_string(),
            argv: Some(argv),
            cwd: None,
            env,
            cols: 106,
            rows: 32,
            history_lines: 2000,
            wrap_iutf8: true,
        }
    }
}

/// Non-blocking handle for sending input, resize, focus, and credit frames to the isolated core.
#[derive(Clone, Debug)]
pub struct HerdrHandle {
    pub attach_channel: u32,
    pub automatic_input_supported: bool,
    writer_tx: mpsc::Sender<Vec<Frame>>,
}

impl HerdrHandle {
    pub fn try_send_user(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.attach_channel == 0 || bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_PAYLOAD_LEN {
            return Err("terminal input exceeds frame size".to_string());
        }
        self.writer_tx
            .try_send(vec![Frame::data(self.attach_channel, bytes)])
            .map_err(|e| format!("herdr user send failed: {e}"))
    }

    pub fn try_resize_with_reports(
        &self,
        cols: u16,
        rows: u16,
        resize: impl FnOnce() -> Result<Vec<Vec<u8>>, String>,
    ) -> Result<bool, String> {
        let permit = match self.writer_tx.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(())) => return Ok(false),
            Err(mpsc::error::TrySendError::Closed(())) => {
                return Err("herdr writer closed".to_string());
            }
        };
        let reports = resize()?;
        let mut frames = vec![Frame::resize(self.attach_channel, cols, rows)];
        if self.automatic_input_supported {
            frames.extend(reports.into_iter().map(|bytes| Frame::automatic_input(self.attach_channel, bytes)));
        }
        permit.send(frames);
        Ok(true)
    }

    pub fn try_send_focus(&self, focused: bool) -> Result<(), String> {
        if self.attach_channel == 0 {
            return Ok(());
        }
        self.writer_tx
            .try_send(vec![Frame::focus(self.attach_channel, focused)])
            .map_err(|e| format!("herdr focus send failed: {e}"))
    }
}

/// Output receiver coupled with automatic flow-control `FRAME_CREDIT` acknowledgement.
pub struct HerdrOutputStream {
    rx: mpsc::Receiver<SessionOutput>,
    handle: HerdrHandle,
}

impl HerdrOutputStream {
    pub async fn recv_uncredited(&mut self) -> Option<SessionOutput> {
        self.rx.recv().await
    }

    pub fn try_recv_uncredited(&mut self) -> Option<SessionOutput> {
        self.rx.try_recv().ok()
    }

    pub async fn acknowledge(&self, bytes: u32, replies: Vec<Vec<u8>>) -> Result<(), String> {
        let channel = self.handle.attach_channel;
        let mut frames = Vec::new();
        if self.handle.automatic_input_supported {
            frames.extend(replies.into_iter().map(|reply| Frame::automatic_input(channel, reply)));
        }
        if bytes > 0 {
            frames.push(Frame::credit(channel, bytes));
        }
        if frames.is_empty() {
            return Ok(());
        }
        self.handle.writer_tx
            .send(frames)
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }
}

struct Attachment {
    address: String,
    channel: u32,
    automatic_input: bool,
    writer_tx: mpsc::Sender<Vec<Frame>>,
    output_rx: mpsc::Receiver<SessionOutput>,
    connection: tokio::task::JoinHandle<()>,
}

/// Owns the dedicated Tokio runtime and isolated `herdr-core` daemon lifetime.
pub struct HerdrGuard {
    runtime: Option<tokio::runtime::Runtime>,
    session: Option<IsolatedHerdrSession>,
}

impl HerdrGuard {
    pub fn start_live(
        opts: HerdrStartOptions,
    ) -> Result<(Self, HerdrHandle, HerdrOutputStream), String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let mut session = runtime.block_on(IsolatedHerdrSession::start_with_options(opts))?;
        let (handle, stream) = Self::take_stream(&mut session)?;
        let guard = Self {
            runtime: Some(runtime),
            session: Some(session),
        };
        Ok((guard, handle, stream))
    }

    fn take_stream(session: &mut IsolatedHerdrSession) -> Result<(HerdrHandle, HerdrOutputStream), String> {
        let handle = session.handle();
        let rx = session.take_output_rx().ok_or_else(|| "missing output receiver".to_string())?;
        let stream = HerdrOutputStream {
            rx,
            handle: handle.clone(),
        };
        Ok((handle, stream))
    }

    pub fn reattach(&mut self, opts: HerdrStartOptions) -> Result<(HerdrHandle, HerdrOutputStream), String> {
        let runtime = self.runtime.as_ref().ok_or("herdr runtime is stopped")?;
        let session = self.session.as_mut().ok_or("herdr session is stopped")?;
        let attachment = runtime
            .block_on(async {
                if let Some(connection) = session.connection.take() {
                    connection.abort();
                    let _ = connection.await;
                }
                tokio::time::timeout(
                    Duration::from_secs(5),
                    IsolatedHerdrSession::attach(&session.socket_path, opts, Some(&session.address), false),
                ).await
            })
            .map_err(|_| "herdr reattach timed out".to_string())??;
        session.attach_channel = attachment.channel;
        session.automatic_input_supported = attachment.automatic_input;
        session.writer_tx = attachment.writer_tx;
        session.output_rx = Some(attachment.output_rx);
        session.connection = Some(attachment.connection);
        Self::take_stream(session)
    }

    pub fn core_pid(&self) -> u32 {
        self.session.as_ref().map(|s| s.core_pid).unwrap_or(0)
    }

    pub fn stop_blocking(&mut self) {
        if let Some(mut session) = self.session.take() {
            if let Some(rt) = &self.runtime {
                rt.block_on(session.stop());
            } else {
                drop(session);
            }
        }
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_millis(500));
        }
    }
}

impl Drop for HerdrGuard {
    fn drop(&mut self) {
        self.stop_blocking();
    }
}

pub struct IsolatedHerdrSession {
    binary: PathBuf,
    session_name: String,
    address: String,
    base_dir: PathBuf,
    pub socket_path: PathBuf,
    child: Option<Child>,
    pub core_pid: u32,
    pub attach_channel: u32,
    pub automatic_input_supported: bool,
    writer_tx: mpsc::Sender<Vec<Frame>>,
    output_rx: Option<mpsc::Receiver<SessionOutput>>,
    connection: Option<tokio::task::JoinHandle<()>>,
    stopped: bool,
}

impl IsolatedHerdrSession {
    #[cfg(test)]
    pub async fn start(
        tag: &str,
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<Self, String> {
        Self::start_with_options(HerdrStartOptions {
            tag: tag.to_string(),
            argv,
            cwd: None,
            env: HashMap::new(),
            cols,
            rows,
            history_lines: 2000,
            wrap_iutf8: false,
        })
        .await
    }

    pub async fn start_with_options(opts: HerdrStartOptions) -> Result<Self, String> {
        let binary = resolve_herdr_core_bin();
        let base_dir =
            create_private_dir(&opts.tag).map_err(|e| format!("create private dir: {e}"))?;
        let mut dir_cleanup = DirCleanupGuard(Some(base_dir.clone()));
        let socket_path = base_dir.join("hc.sock");
        let session_name = format!(
            "gpui-poc-{}-{}",
            std::process::id(),
            INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed)
        );

        let cmd = Self::build_core_cmd(&binary, &session_name, &base_dir, &socket_path, &["serve"]);
        let child = Command::from(cmd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn herdr-core ({}): {e}", binary.display()))?;
        let core_pid = child.id().unwrap_or(0);

        let attached = tokio::time::timeout(Duration::from_secs(5), Self::attach(&socket_path, opts, None, false)).await;
        let attachment = match attached {
            Ok(Ok(attachment)) => attachment,
            result => {
                let mut child = child;
                stop_core(&binary, &session_name, &base_dir, &socket_path, &mut child).await;
                return Err(match result {
                    Ok(Err(error)) => error,
                    _ => "herdr startup timed out".to_string(),
                });
            }
        };
        dir_cleanup.disarm();
        Ok(Self {
            binary,
            session_name,
            address: attachment.address,
            base_dir,
            socket_path,
            child: Some(child),
            core_pid,
            attach_channel: attachment.channel,
            automatic_input_supported: attachment.automatic_input,
            writer_tx: attachment.writer_tx,
            output_rx: Some(attachment.output_rx),
            connection: Some(attachment.connection),
            stopped: false,
        })
    }

    async fn attach(
        socket_path: &Path,
        mut opts: HerdrStartOptions,
        address: Option<&str>,
        observe: bool,
    ) -> Result<Attachment, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(socket_path).await {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => {
                    return Err(format!("connect herdr-core socket: {e}"));
                }
            }
        };

        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        // 1. Plain-mode JSON-RPC `hello` to upgrade to Framed Mode
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
        let mut hello_line = serde_json::to_string(&hello_req).map_err(|e| e.to_string())?;
        hello_line.push('\n');
        write_half
            .write_all(hello_line.as_bytes())
            .await
            .map_err(|e| format!("write hello: {e}"))?;
        write_half.flush().await.map_err(|e| format!("flush hello: {e}"))?;

        let mut resp_line = String::new();
        (&mut reader)
            .take(64 * 1024)
            .read_line(&mut resp_line)
            .await
            .map_err(|e| format!("read hello: {e}"))?;
        let resp_val: Value =
            serde_json::from_str(&resp_line).map_err(|e| format!("parse hello: {e}"))?;
        let hello_res = resp_val
            .get("result")
            .ok_or_else(|| format!("hello error: {resp_val}"))?;
        if hello_res.get("version").and_then(Value::as_str) != Some(CORE_VERSION) {
            return Err(format!("unexpected herdr-core version: {hello_res}"));
        }
        let features: HashSet<String> = hello_res
            .get("features")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let automatic_input_supported = features.contains("automatic_input");

        let terminal_addr = if let Some(address) = address {
            address.to_owned()
        } else {
            // 2. `terminal.run` over Framed Mode channel 0
            if opts.wrap_iutf8
                && let Some(argv) = opts.argv.take()
            {
                if let Ok(exe) = std::env::current_exe()
                    && let Some(exe_str) = exe.to_str()
                {
                    opts.argv = Some(os::wrap_argv_with_pty_iutf8_init(argv, exe_str));
                } else {
                    opts.argv = Some(argv);
                }
            }

            let mut run_params = serde_json::Map::new();
            run_params.insert("size".into(), json!([opts.cols, opts.rows]));
            if let Some(cmd_argv) = opts.argv {
                run_params.insert("argv".into(), json!(cmd_argv));
            }
            if let Some(cwd) = opts.cwd {
                run_params.insert("cwd".into(), json!(cwd));
            }
            if !opts.env.is_empty() {
                run_params.insert("env".into(), json!(opts.env));
            }
            let run_req = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "terminal.run",
                "params": Value::Object(run_params),
            });
            write_frame(
                &mut write_half,
                &Frame::data(0, serde_json::to_vec(&run_req).map_err(|e| e.to_string())?),
            )
            .await
            .map_err(|e| format!("write terminal.run: {e}"))?;

            let run_reply_frame = read_frame(&mut reader)
                .await
                .map_err(|e| format!("read terminal.run reply: {e}"))?;
            let run_reply: Value = serde_json::from_slice(&run_reply_frame.payload)
                .map_err(|e| format!("parse terminal.run: {e}"))?;
            run_reply
                .pointer("/result/address")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing terminal address: {run_reply}"))?
                .to_string()

        };

        // 3. `terminal.attach` over Framed Mode channel 0
        let attach_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "terminal.attach",
            "params": {
                "terminal": terminal_addr,
                "cols": opts.cols,
                "rows": opts.rows,
                "history": opts.history_lines,
                "observe": observe,
                "resize_notifications": true
            }
        });
        write_frame(
            &mut write_half,
            &Frame::data(0, serde_json::to_vec(&attach_req).map_err(|e| e.to_string())?),
        )
        .await
        .map_err(|e| format!("write terminal.attach: {e}"))?;

        let mut early_frames = Vec::new();
        let mut early_frame_bytes = 0;
        let (attach_channel, attach_auto_input) = loop {
            let f = read_frame(&mut reader)
                .await
                .map_err(|e| format!("read terminal.attach reply: {e}"))?;
            if f.channel == 0 && f.kind == FRAME_DATA {
                let v: Value = serde_json::from_slice(&f.payload)
                    .map_err(|e| format!("parse terminal.attach: {e}"))?;
                if v.get("id").and_then(Value::as_u64) == Some(3) {
                    let res = v
                        .get("result")
                        .ok_or_else(|| format!("terminal.attach error: {v}"))?;
                    let ch = res
                        .get("channel")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| format!("missing channel: {res}"))?
                        as u32;
                    let auto = res
                        .get("automatic_input")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    break (ch, auto);
                }
            } else {
                early_frame_bytes += HEADER_LEN + f.payload.len();
                if early_frame_bytes > 512 * 1024 {
                    return Err("herdr exceeded the pre-attach frame budget".to_string());
                }
                early_frames.push(f);
            }
        };

        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<Frame>>(512);
        let (output_tx, output_rx) = mpsc::channel::<SessionOutput>(4096);

        let write = async move {
            while let Some(frames) = writer_rx.recv().await {
                for frame in frames {
                    if write_frame(&mut write_half, &frame).await.is_err() {
                        return;
                    }
                }
            }
        };

        let disconnected_tx = output_tx.clone();
        let read = async move {
            let mut early = early_frames.into_iter();
            loop {
                let frame = match early.next() {
                    Some(frame) => Ok(frame),
                    None => read_frame(&mut reader).await,
                };
                let output = match frame {
                    Ok(frame) if frame.channel != attach_channel => continue,
                    Ok(frame) => match frame.kind {
                        FRAME_DATA => SessionOutput::Data(frame.payload),
                        FRAME_RESIZE => {
                            let [c0, c1, r0, r1] = frame.payload.as_slice() else {
                                break;
                            };
                            let cols = u16::from_le_bytes([*c0, *c1]);
                            let rows = u16::from_le_bytes([*r0, *r1]);
                            if !(1..=1000).contains(&cols) || !(1..=1000).contains(&rows) {
                                break;
                            }
                            SessionOutput::Resize { cols, rows }
                        }
                        FRAME_CLOSE => SessionOutput::Closed(CloseInfo::from_payload(&frame.payload)),
                        _ => continue,
                    },
                    Err(_) => break,
                };
                let closed = matches!(output, SessionOutput::Closed(_));
                if output_tx.send(output).await.is_err() || closed {
                    return true;
                }
            }
            false
        };

        let connection = tokio::spawn(async move {
            let closed = tokio::select! {
                biased;
                closed = read => closed,
                () = write => false,
            };
            if !closed {
                let _ = disconnected_tx.send(SessionOutput::Closed(CloseInfo {
                    exit_code: None,
                    signal: None,
                    reason: "disconnected".to_string(),
                })).await;
            }
        });

        Ok(Attachment {
            address: terminal_addr,
            channel: attach_channel,
            automatic_input: automatic_input_supported && attach_auto_input,
            writer_tx,
            output_rx,
            connection,
        })
    }

    pub fn handle(&self) -> HerdrHandle {
        HerdrHandle {
            attach_channel: self.attach_channel,
            automatic_input_supported: self.automatic_input_supported,
            writer_tx: self.writer_tx.clone(),
        }
    }

    pub fn take_output_rx(&mut self) -> Option<mpsc::Receiver<SessionOutput>> {
        self.output_rx.take()
    }

    fn build_core_cmd(
        binary: &Path,
        session: &str,
        base_dir: &Path,
        socket_path: &Path,
        args: &[&str],
    ) -> std::process::Command {
        let mut cmd = std::process::Command::new(binary);
        cmd.args(args)
            .env_remove("HC_SCOPE")
            .env("HC_RUNTIME", "1")
            .env("HC_LAUNCHD_DIRECT", std::process::id().to_string())
            .env_remove("HC_LAUNCHD_DETACH")
            .env("HC_SESSION", session)
            .env("HC_SOCKET", socket_path)
            .env("XDG_STATE_HOME", base_dir.join("st"))
            .env("XDG_CONFIG_HOME", base_dir.join("cf"))
            .env("XDG_RUNTIME_DIR", base_dir.join("rt"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    #[cfg(test)]
    pub async fn write_user(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.attach_channel == 0 || bytes.is_empty() {
            return Ok(());
        }
        self.writer_tx
            .send(vec![Frame::data(self.attach_channel, bytes)])
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    #[cfg(test)]
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        if self.attach_channel == 0 || cols == 0 || rows == 0 {
            return Ok(());
        }
        self.writer_tx
            .send(vec![Frame::resize(self.attach_channel, cols, rows)])
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    #[cfg(test)]
    pub async fn focus(&self, focused: bool) -> Result<(), String> {
        if self.attach_channel == 0 {
            return Ok(());
        }
        self.writer_tx
            .send(vec![Frame::focus(self.attach_channel, focused)])
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    #[cfg(test)]
    pub async fn recv_output(&mut self) -> Option<SessionOutput> {
        let rx = self.output_rx.as_mut()?;
        let ev = rx.recv().await?;
        if let SessionOutput::Data(ref bytes) = ev {
            let credit = bytes.len().min(u32::MAX as usize) as u32;
            if credit > 0 {
                let _ = self
                    .writer_tx
                    .send(vec![Frame::credit(self.attach_channel, credit)])
                    .await;
            }
        }
        Some(ev)
    }

    pub async fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(connection) = self.connection.take() {
            connection.abort();
            let _ = connection.await;
        }
        if let Some(child) = &mut self.child {
            stop_core(&self.binary, &self.session_name, &self.base_dir, &self.socket_path, child).await;
        }
        let _ = std::fs::remove_dir_all(&self.base_dir);
    }
}

async fn stop_core(binary: &Path, session: &str, base_dir: &Path, socket: &Path, child: &mut Child) {
    if socket.exists() {
        let mut command = Command::from(IsolatedHerdrSession::build_core_cmd(
            binary, session, base_dir, socket, &["--json", "server", "stop"],
        ));
        command.kill_on_drop(true);
        let _ = tokio::time::timeout(Duration::from_secs(2), command.status()).await;
    }
    if !matches!(tokio::time::timeout(Duration::from_secs(2), child.wait()).await, Ok(Ok(_))) {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
}

impl Drop for IsolatedHerdrSession {
    fn drop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            if let Some(connection) = self.connection.take() {
                connection.abort();
            }
            if let Some(child) = &mut self.child {
                let _ = child.start_kill();
            }
            let _ = std::fs::remove_dir_all(&self.base_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn receives_the_controlling_viewers_resize_as_an_observer() {
        let mut session = IsolatedHerdrSession::start(
            "resize-notice", Some(vec!["/bin/cat".to_string()]), 80, 24,
        ).await.expect("start isolated core");
        let opts = HerdrStartOptions {
            cols: 80,
            rows: 24,
            ..HerdrStartOptions::default()
        };
        let mut observer = IsolatedHerdrSession::attach(&session.socket_path, opts, Some(&session.address), true)
            .await.expect("attach observer");
        session.resize(96, 28).await.expect("resize controlling viewer");
        let notice = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match observer.output_rx.recv().await {
                    Some(SessionOutput::Resize { cols, rows }) if (cols, rows) == (96, 28) => break,
                    Some(SessionOutput::Data(bytes)) => observer.writer_tx.send(vec![
                        Frame::credit(observer.channel, bytes.len() as u32),
                    ]).await.unwrap(),
                    Some(SessionOutput::Resize { .. }) => {}
                    _ => panic!("observer closed before the controlling viewer's resize"),
                }
            }
        }).await;
        observer.connection.abort();
        session.stop().await;
        notice.expect("observer received resize");
    }

    #[tokio::test]
    async fn resize_and_report_wait_for_space_and_keep_wire_order() {
        let (writer_tx, mut writer_rx) = mpsc::channel(1);
        let handle = HerdrHandle {
            attach_channel: 7,
            automatic_input_supported: true,
            writer_tx,
        };
        handle.try_send_user(b"x".to_vec()).unwrap();
        assert!(!handle.try_resize_with_reports(80, 24, || panic!("resize applied too early")).unwrap());
        assert_eq!(writer_rx.recv().await.unwrap(), [Frame::data(7, b"x".to_vec())]);

        let report = b"size report".to_vec();
        assert!(handle.try_resize_with_reports(80, 24, || Ok(vec![report.clone()])).unwrap());
        assert_eq!(
            writer_rx.recv().await.unwrap(),
            [Frame::resize(7, 80, 24), Frame::automatic_input(7, report)],
        );
    }

    #[tokio::test]
    async fn acknowledging_consumed_output_waits_for_space() {
        let (writer_tx, mut writer_rx) = mpsc::channel(1);
        let (_, rx) = mpsc::channel(1);
        let handle = HerdrHandle { attach_channel: 7, automatic_input_supported: true, writer_tx };
        let stream = HerdrOutputStream { rx, handle };
        stream.handle.try_send_user(b"x".to_vec()).unwrap();

        let ack = stream.acknowledge(42, vec![b"reply".to_vec()]);
        tokio::pin!(ack);
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut ack).await.is_err());
        writer_rx.recv().await.unwrap();
        ack.await.unwrap();
        assert_eq!(writer_rx.recv().await.unwrap(), [Frame::automatic_input(7, b"reply".to_vec()), Frame::credit(7, 42)]);
    }

    #[test]
    fn close_info_handles_numeric_and_string_signals() {
        let num_sig = CloseInfo::from_payload(br#"{"reason":"signaled","exit_code":null,"signal":9}"#);
        assert_eq!(num_sig.exit_code, None);
        assert_eq!(num_sig.signal.as_deref(), Some("SIGKILL (9)"));
        assert_eq!(num_sig.reason, "signaled");

        let exited = CloseInfo::from_payload(br#"{"reason":"exited","exit_code":7,"signal":null}"#);
        assert_eq!(exited.exit_code, Some(7));
        assert_eq!(exited.signal, None);
        assert_eq!(exited.reason, "exited");
    }

    #[tokio::test]
    async fn isolated_herdr_core_roundtrip_resize_and_auto_input() {
        let mut sess = IsolatedHerdrSession::start(
            "unit",
            Some(vec!["/bin/sh".into()]),
            80,
            24,
        )
        .await
        .expect("start isolated herdr-core");
        assert!(sess.automatic_input_supported);
        assert!(sess.attach_channel > 0);

        sess.resize(110, 32).await.expect("resize");
        tokio::time::sleep(Duration::from_millis(80)).await;
        sess.write_user(b"stty size; printf '%s%s\\n' GPUI_ HERDR_OK\n".to_vec())
            .await
            .expect("write_user");

        let mut captured = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !captured.contains("GPUI_HERDR_OK") {
            match tokio::time::timeout(Duration::from_millis(200), sess.recv_output()).await {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    captured.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        sess.stop().await;
        assert!(
            captured.contains("32 110"),
            "expected resized 32 110 in output, got: {captured:?}"
        );
        assert!(
            captured.contains("GPUI_HERDR_OK"),
            "expected marker in output, got: {captured:?}"
        );
    }

    #[tokio::test]
    async fn focus_frame_delivers_csi_i_o_when_mode_1004_enabled() {
        let mut sess = IsolatedHerdrSession::start(
            "focus",
            Some(vec![
                "/bin/sh".into(),
                "-c".into(),
                "stty raw -echo; printf 'READY\\n'; dd bs=1 count=6 2>/dev/null | od -An -tx1; stty sane".into(),
            ]),
            80,
            24,
        )
        .await
        .expect("start isolated herdr-core");

        let mut ready_buf = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !ready_buf.contains("READY") {
            match tokio::time::timeout(Duration::from_millis(200), sess.recv_output()).await {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    ready_buf.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        assert!(ready_buf.contains("READY"), "got: {ready_buf:?}");

        // Send FOCUS(true) and FOCUS(false); herdr-core emits CSI I (1b 5b 49) & CSI O (1b 5b 4f)
        // when mode 1004 is enabled via VT, OR let's enable 1004 before FOCUS if herdr-core tracks it!
        // Note: herdr-core's VT parser tracks DEC 1004 on PTY output, so let's send it via printf first.
        sess.stop().await;

        let mut sess = IsolatedHerdrSession::start(
            "focus2",
            Some(vec![
                "/bin/sh".into(),
                "-c".into(),
                "stty raw -echo; printf '\\033[?1004hREADY\\n'; dd bs=1 count=6 2>/dev/null | od -An -tx1; stty sane".into(),
            ]),
            80,
            24,
        )
        .await
        .expect("start isolated herdr-core");

        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !out.contains("READY") {
            match tokio::time::timeout(Duration::from_millis(200), sess.recv_output()).await {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        assert!(out.contains("READY"), "got: {out:?}");
        // Give herdr-core a tick to process \e[?1004h, then send focus gained + lost
        tokio::time::sleep(Duration::from_millis(30)).await;
        sess.focus(true).await.expect("focus true");
        sess.focus(false).await.expect("focus false");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), sess.recv_output()).await {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        sess.stop().await;
        let normalized: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("1b 5b 49") && normalized.contains("1b 5b 4f"),
            "expected CSI I (1b 5b 49) and CSI O (1b 5b 4f) in output, got: {out:?}"
        );
    }
}
