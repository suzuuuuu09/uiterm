use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Frontend-facing IPC error payload (camelCase JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub hint: Option<String>,
}

impl IpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint,
        }
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DisplayFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub cell: Option<[u32; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub version: String,
    pub pid: u32,
    pub uptime_ms: u64,
    pub live: u64,
    pub finished: u64,
    pub session: String,
    pub socket_path: String,
    pub automatic_input_supported: bool,
    pub resize_notifications_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ForegroundProcess {
    pub pid: i32,
    pub name: String,
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatusRecord {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional = nullable))]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[cfg_attr(test, ts(type = "string", optional))]
    pub app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[cfg_attr(test, ts(type = "string", optional))]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[cfg_attr(test, ts(type = "string", optional))]
    pub msg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub address: String,
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub state: String,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
    pub pid: Option<i32>,
    pub created_ms: u64,
    pub finished_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub reason: Option<String>,
    pub status: Option<StatusRecord>,
    pub status_ms: Option<u64>,
    pub turn: u64,
    pub labels: HashMap<String, String>,
    pub foreground: Option<ForegroundProcess>,
}

/// Raw herdr-core Terminal JSON (snake_case) -> frontend TerminalInfo (camelCase).
#[derive(Debug, Clone, Deserialize)]
pub struct HerdrTerminalRecord {
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pid: Option<i32>,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub status: Option<StatusRecord>,
    #[serde(default)]
    pub status_ms: Option<u64>,
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub foreground: Option<ForegroundProcess>,
}

impl From<HerdrTerminalRecord> for TerminalInfo {
    fn from(r: HerdrTerminalRecord) -> Self {
        Self {
            address: r.address,
            name: r.name,
            argv: crate::os::strip_pty_iutf8_wrapper(r.argv),
            cwd: r.cwd,
            state: r.state,
            cols: r.cols,
            rows: r.rows,
            title: r.title,
            pid: r.pid,
            created_ms: r.created_ms,
            finished_ms: r.finished_ms,
            exit_code: r.exit_code,
            signal: r.signal.map(|signal| signal.to_string()),
            reason: r.reason,
            status: r.status,
            status_ms: r.status_ms,
            turn: r.turn,
            labels: r.labels,
            foreground: r.foreground,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateParams {
    #[cfg_attr(test, ts(optional = nullable))]
    pub argv: Option<Vec<String>>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub line: Option<String>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub cwd: Option<String>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub env: Option<HashMap<String, String>>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub name: Option<String>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub labels: Option<HashMap<String, String>>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub cols: Option<u16>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub rows: Option<u16>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub display: Option<DisplayFacts>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub keep: Option<bool>,
}

impl TerminalCreateParams {
    pub(crate) fn validate(&self) -> Result<(), IpcError> {
        let invalid = |message| IpcError::new("invalid_args", message);
        if self.argv.is_some() && self.line.is_some() {
            return Err(invalid("specify either argv or line"));
        }
        if let Some(argv) = &self.argv
            && (argv.first().is_none_or(String::is_empty)
                || argv.iter().any(|arg| arg.contains('\0')))
        {
            return Err(invalid(
                "argv requires a nonempty program and NUL-free arguments",
            ));
        }
        if self
            .line
            .as_ref()
            .is_some_and(|line| line.trim().is_empty() || line.contains('\0'))
        {
            return Err(invalid("line must contain a command without NUL"));
        }
        if self.cwd.as_ref().is_some_and(|cwd| cwd.contains('\0')) {
            return Err(invalid("cwd must not contain NUL"));
        }
        if self.env.as_ref().is_some_and(|env| {
            env.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
            })
        }) {
            return Err(invalid("invalid environment entry"));
        }
        if self.cols.is_some() != self.rows.is_some() {
            return Err(invalid("specify both terminal dimensions"));
        }
        if let (Some(cols), Some(rows)) = (self.cols, self.rows) {
            super::client::validate_size(cols, rows)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachParams {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
    #[cfg_attr(test, ts(optional = nullable))]
    pub history: Option<u32>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub observe: Option<bool>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub resize_notifications: Option<bool>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub display: Option<DisplayFacts>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachResult {
    pub attach_id: u32,
    pub terminal: TerminalInfo,
    pub automatic_input: bool,
    pub resize_notifications: bool,
    pub initial_credit_bytes: u32,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EventsSubscribeResult {
    pub subscription_id: u32,
    pub seq: u64,
    pub terminals: Vec<TerminalInfo>,
}

/// Binary stream frame delivered over Tauri Channel<Response>.
pub const OUTPUT_FRAME_DATA: u8 = 0x00;
pub const OUTPUT_FRAME_RESIZE: u8 = 0x01;
pub const OUTPUT_FRAME_CLOSE: u8 = 0x02;

#[derive(Debug, Clone)]
pub enum OutputStreamFrame {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close(Vec<u8>),
}

impl OutputStreamFrame {
    /// Encode into the `[tag: u8][payload...]` binary format defined in docs/poc/interface.md.
    pub fn into_ipc_bytes(self) -> Vec<u8> {
        match self {
            Self::Data(bytes) => {
                let mut out = Vec::with_capacity(1 + bytes.len());
                out.push(OUTPUT_FRAME_DATA);
                out.extend_from_slice(&bytes);
                out
            }
            Self::Resize { cols, rows } => {
                let mut out = Vec::with_capacity(5);
                out.push(OUTPUT_FRAME_RESIZE);
                out.extend_from_slice(&cols.to_le_bytes());
                out.extend_from_slice(&rows.to_le_bytes());
                out
            }
            Self::Close(json_bytes) => {
                let mut out = Vec::with_capacity(1 + json_bytes.len());
                out.push(OUTPUT_FRAME_CLOSE);
                out.extend_from_slice(&json_bytes);
                out
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CoreEvent {
    Created {
        seq: u64,
        terminal: String,
        data: TerminalInfo,
    },
    Lost {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Restarted {
        seq: u64,
        terminal: String,
        data: TerminalInfo,
    },
    Finished {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Removed {
        seq: u64,
        terminal: String,
    },
    Status {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Turn {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Title {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Size {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Foreground {
        seq: u64,
        terminal: String,
        data: Option<ForegroundProcess>,
    },
    Labels {
        seq: u64,
        terminal: String,
        #[cfg_attr(test, ts(type = "unknown"))]
        data: Value,
    },
    Heartbeat {
        seq: u64,
    },
    Missed {
        seq: u64,
        terminals: Vec<TerminalInfo>,
    },
    Disconnected {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_create_params_validate_accepts_valid_variants() {
        assert!(TerminalCreateParams::default().validate().is_ok());

        assert!(
            TerminalCreateParams {
                argv: Some(vec!["/bin/sh".into(), "-c".into(), "echo ok".into()]),
                cwd: Some("/tmp".into()),
                env: Some(HashMap::from([("TERM".into(), "xterm-256color".into())])),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );

        assert!(
            TerminalCreateParams {
                line: Some("echo hello".into()),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn terminal_create_params_validate_rejects_invalid_inputs() {
        let cases = [
            // 1. argv and line both set
            TerminalCreateParams {
                argv: Some(vec!["/bin/sh".into()]),
                line: Some("echo hi".into()),
                ..Default::default()
            },
            // 2a. empty argv
            TerminalCreateParams {
                argv: Some(vec![]),
                ..Default::default()
            },
            // 2b. empty program in argv
            TerminalCreateParams {
                argv: Some(vec!["".into()]),
                ..Default::default()
            },
            // 2c. NUL byte in argv
            TerminalCreateParams {
                argv: Some(vec!["/bin/sh".into(), "a\0b".into()]),
                ..Default::default()
            },
            // 3a. whitespace-only line
            TerminalCreateParams {
                line: Some("   ".into()),
                ..Default::default()
            },
            // 3b. NUL byte in line
            TerminalCreateParams {
                line: Some("echo \0".into()),
                ..Default::default()
            },
            // 4. NUL byte in cwd
            TerminalCreateParams {
                cwd: Some("/tmp/\0bad".into()),
                ..Default::default()
            },
            // 5a. empty env key
            TerminalCreateParams {
                env: Some(HashMap::from([("".into(), "1".into())])),
                ..Default::default()
            },
            // 5b. '=' in env key
            TerminalCreateParams {
                env: Some(HashMap::from([("A=B".into(), "1".into())])),
                ..Default::default()
            },
            // 5c. NUL in env value
            TerminalCreateParams {
                env: Some(HashMap::from([("KEY".into(), "v\0".into())])),
                ..Default::default()
            },
            // 6. only one dimension specified
            TerminalCreateParams {
                cols: Some(80),
                ..Default::default()
            },
            TerminalCreateParams {
                rows: Some(24),
                ..Default::default()
            },
            // 7. out-of-range dimensions
            TerminalCreateParams {
                cols: Some(0),
                rows: Some(24),
                ..Default::default()
            },
        ];

        for (idx, params) in cases.iter().enumerate() {
            let err = params
                .validate()
                .expect_err(&format!("case {idx} should fail validation"));
            assert_eq!(err.code, "invalid_args", "case {idx} error code mismatch");
        }
    }
}
