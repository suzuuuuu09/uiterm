//! Isolated `herdr-core` v0.1.22 Framed Mode client for the GPUI PoC.
//!
//! Strictly obeys AGENTS.md invariants:
//! - Always spawns a dedicated `herdr-core` in a private `0700` directory and socket.
//! - Never connects to the default Herdr socket or stops unrelated instances.
//! - Separates user input (`FRAME_DATA`, kind 0) from VT auto-responses
//!   (`FRAME_AUTOMATIC_INPUT`, kind 6) without ever falling back to `FRAME_DATA`.
//! - Never logs raw terminal input bytes or IME strings.

use std::collections::HashSet;
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

pub const CORE_VERSION: &str = "0.1.22";
pub const HEADER_LEN: usize = 9;
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
pub const CREDIT_ACK_THRESHOLD: u32 = 16 * 1024;

pub const FRAME_DATA: u8 = 0;
pub const FRAME_CREDIT: u8 = 1;
pub const FRAME_CLOSE: u8 = 2;
#[allow(dead_code)]
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

    #[allow(dead_code)]
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
    PathBuf::from("herdr-core")
}

fn create_private_dir(tag: &str) -> io::Result<PathBuf> {
    let seq = INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    // Keep Unix domain socket path well below macOS SUN_LEN (104 bytes).
    let dir = PathBuf::from(format!("/tmp/u-gp-{tag}-{pid}-{seq}"));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

#[derive(Debug)]
pub enum SessionOutput {
    Data(Vec<u8>),
    Closed(#[allow(dead_code)] String),
}

pub struct IsolatedHerdrSession {
    binary: PathBuf,
    session_name: String,
    base_dir: PathBuf,
    socket_path: PathBuf,
    child: Option<Child>,
    pub core_pid: u32,
    pub attach_channel: u32,
    pub automatic_input_supported: bool,
    writer_tx: mpsc::Sender<Frame>,
    output_rx: mpsc::Receiver<SessionOutput>,
    pending_ack_bytes: u32,
    stopped: bool,
}

impl IsolatedHerdrSession {
    pub async fn start(
        tag: &str,
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
    ) -> Result<Self, String> {
        let binary = resolve_herdr_core_bin();
        let base_dir = create_private_dir(tag).map_err(|e| format!("create private dir: {e}"))?;
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

        // Wait for private socket readiness
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(&socket_path).await {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&base_dir);
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

        // 2. `terminal.run` over Framed Mode channel 0
        let mut run_params = serde_json::Map::new();
        run_params.insert("size".into(), json!([cols, rows]));
        if let Some(cmd_argv) = argv {
            run_params.insert("argv".into(), json!(cmd_argv));
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
        let terminal_addr = run_reply
            .pointer("/result/address")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing terminal address: {run_reply}"))?
            .to_string();

        // 3. `terminal.attach` over Framed Mode channel 0
        let attach_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "terminal.attach",
            "params": {
                "terminal": terminal_addr,
                "cols": cols,
                "rows": rows,
                "history": 2000
            }
        });
        write_frame(
            &mut write_half,
            &Frame::data(0, serde_json::to_vec(&attach_req).map_err(|e| e.to_string())?),
        )
        .await
        .map_err(|e| format!("write terminal.attach: {e}"))?;

        // Drain frames until channel 0 JSON-RPC reply id=3 arrives (buffering any early stream frames)
        let mut early_frames = Vec::new();
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
                early_frames.push(f);
            }
        };

        let (writer_tx, mut writer_rx) = mpsc::channel::<Frame>(256);
        let (output_tx, output_rx) = mpsc::channel::<SessionOutput>(4096);

        // Writer task
        tokio::spawn(async move {
            while let Some(frame) = writer_rx.recv().await {
                if write_frame(&mut write_half, &frame).await.is_err() {
                    break;
                }
            }
        });

        // Reader task
        tokio::spawn(async move {
            for f in early_frames {
                if f.channel == attach_channel {
                    match f.kind {
                        FRAME_DATA => {
                            let _ = output_tx.send(SessionOutput::Data(f.payload)).await;
                        }
                        FRAME_CLOSE => {
                            let reason = String::from_utf8_lossy(&f.payload).into_owned();
                            let _ = output_tx.send(SessionOutput::Closed(reason)).await;
                            return;
                        }
                        _ => {}
                    }
                }
            }
            loop {
                match read_frame(&mut reader).await {
                    Ok(f) if f.channel == attach_channel => match f.kind {
                        FRAME_DATA => {
                            if output_tx.send(SessionOutput::Data(f.payload)).await.is_err() {
                                break;
                            }
                        }
                        FRAME_CLOSE => {
                            let reason = String::from_utf8_lossy(&f.payload).into_owned();
                            let _ = output_tx.send(SessionOutput::Closed(reason)).await;
                            break;
                        }
                        _ => {}
                    },
                    Ok(_) => {}
                    Err(_) => {
                        let _ = output_tx
                            .send(SessionOutput::Closed("eof".to_string()))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(Self {
            binary,
            session_name,
            base_dir,
            socket_path,
            child: Some(child),
            core_pid,
            attach_channel,
            automatic_input_supported: automatic_input_supported && attach_auto_input,
            writer_tx,
            output_rx,
            pending_ack_bytes: 0,
            stopped: false,
        })
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

    pub async fn write_user(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.attach_channel == 0 || bytes.is_empty() {
            return Ok(());
        }
        self.writer_tx
            .send(Frame::data(self.attach_channel, bytes))
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    pub async fn write_automatic(&self, bytes: Vec<u8>) -> Result<(), String> {
        if self.attach_channel == 0 || bytes.is_empty() {
            return Ok(());
        }
        if !self.automatic_input_supported {
            // Never fall back to FRAME_DATA for automatic responses.
            return Err("automatic_input not negotiated".to_string());
        }
        self.writer_tx
            .send(Frame::automatic_input(self.attach_channel, bytes))
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    #[allow(dead_code)]
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        if self.attach_channel == 0 || cols == 0 || rows == 0 {
            return Ok(());
        }
        self.writer_tx
            .send(Frame::resize(self.attach_channel, cols, rows))
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    #[allow(dead_code)]
    pub async fn focus(&self, focused: bool) -> Result<(), String> {
        if self.attach_channel == 0 {
            return Ok(());
        }
        self.writer_tx
            .send(Frame::focus(self.attach_channel, focused))
            .await
            .map_err(|_| "herdr writer closed".to_string())
    }

    pub async fn recv_output(&mut self) -> Option<SessionOutput> {
        let ev = self.output_rx.recv().await?;
        if let SessionOutput::Data(ref bytes) = ev {
            self.note_consumed(bytes.len()).await;
        }
        Some(ev)
    }

    #[allow(dead_code)]
    pub fn try_recv_output(&mut self) -> Option<SessionOutput> {
        let ev = self.output_rx.try_recv().ok()?;
        if let SessionOutput::Data(ref bytes) = ev {
            self.pending_ack_bytes = self
                .pending_ack_bytes
                .saturating_add(bytes.len().min(u32::MAX as usize) as u32);
            if self.pending_ack_bytes >= CREDIT_ACK_THRESHOLD {
                let credit = std::mem::take(&mut self.pending_ack_bytes);
                let _ = self
                    .writer_tx
                    .try_send(Frame::credit(self.attach_channel, credit));
            }
        }
        Some(ev)
    }

    async fn note_consumed(&mut self, len: usize) {
        self.pending_ack_bytes = self
            .pending_ack_bytes
            .saturating_add(len.min(u32::MAX as usize) as u32);
        if self.pending_ack_bytes >= CREDIT_ACK_THRESHOLD {
            let credit = std::mem::take(&mut self.pending_ack_bytes);
            let _ = self
                .writer_tx
                .send(Frame::credit(self.attach_channel, credit))
                .await;
        }
    }

    pub async fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if self.socket_path.exists() {
            let _ = Command::from(Self::build_core_cmd(
                &self.binary,
                &self.session_name,
                &self.base_dir,
                &self.socket_path,
                &["--json", "server", "stop"],
            ))
            .output()
            .await;
        }
        if let Some(child) = &mut self.child {
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = child.start_kill();
        }
        let _ = std::fs::remove_dir_all(&self.base_dir);
    }
}

impl Drop for IsolatedHerdrSession {
    fn drop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            if self.socket_path.exists() {
                let _ = Self::build_core_cmd(
                    &self.binary,
                    &self.session_name,
                    &self.base_dir,
                    &self.socket_path,
                    &["--json", "server", "stop"],
                )
                .status();
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
                Err(_) => {}
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
}
