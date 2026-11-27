//! Integration & compatibility tests against real isolated `herdr-core` v0.1.22,
//! `uiterm-gpui-poc --uiterm-pty-init`, `fish`, `nvim`, and `codex --version`.

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../src/os.rs"]
mod os;

#[allow(dead_code)]
#[path = "../src/herdr.rs"]
mod herdr;

use herdr::{HerdrGuard, HerdrStartOptions, SessionOutput};

fn has_cmd(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn pty_iutf8_init_enables_utf8_backspace_on_real_herdr_pty() {
    let bin = env!("CARGO_BIN_EXE_uiterm-gpui-poc");
    let wrapped = os::wrap_argv_with_pty_iutf8_init(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "stty -a; cat".into(),
        ],
        bin,
    );

    let opts = HerdrStartOptions {
        tag: "iutf8".into(),
        argv: Some(wrapped),
        cwd: None,
        env: HashMap::from([("LANG".into(), "en_US.UTF-8".into())]),
        cols: 80,
        rows: 24,
        history_lines: 200,
        wrap_iutf8: false,
    };

    let (mut guard, handle, mut stream) = HerdrGuard::start_live(opts).expect("start herdr");
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut out = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !out.contains("iutf8") {
        if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(200), stream.recv()).await
        }) {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    assert!(
        out.contains("iutf8") && !out.contains("-iutf8"),
        "expected iutf8 enabled on fresh herdr PTY, got: {out:?}"
    );

    // Canonical Japanese Backspace test: "あいうえお" + 5x DEL + "かきくけこ\n" -> "かきくけこ"
    let mut seq = "あいうえお".as_bytes().to_vec();
    seq.extend_from_slice(&[0x7f; 5]);
    seq.extend_from_slice("かきくけこ\n".as_bytes());
    seq.extend_from_slice("A日本".as_bytes());
    seq.push(0x7f);
    seq.extend_from_slice("B\n".as_bytes());
    seq.push(0x04); // Ctrl+D EOF
    handle.try_send_user(seq).expect("send canonical input");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(250), stream.recv()).await
        }) {
            Ok(Some(SessionOutput::Data(bytes))) => {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
            Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
            Err(_) => {}
        }
    }
    guard.stop_blocking();

    assert!(
        out.contains("かきくけこ"),
        "expected intact かきくけこ after 5 UTF-8 backspaces, got: {out:?}"
    );
    assert!(
        out.contains("A日B"),
        "expected intact A日B after 1 UTF-8 backspace, got: {out:?}"
    );
}

#[test]
fn installed_clis_fish_nvim_codex_and_resize_altscreen_work_in_isolated_pty() {
    let bin = env!("CARGO_BIN_EXE_uiterm-gpui-poc");

    // 1. Fish shell + resize check
    if has_cmd("fish") {
        let wrapped = os::wrap_argv_with_pty_iutf8_init(vec!["fish".into(), "-l".into()], bin);
        let opts = HerdrStartOptions {
            tag: "fish".into(),
            argv: Some(wrapped),
            cwd: None,
            env: os::build_child_shell_env(Some("fish")),
            cols: 80,
            rows: 24,
            history_lines: 200,
            wrap_iutf8: false,
        };
        let (mut guard, handle, mut stream) = HerdrGuard::start_live(opts).expect("start fish");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(300), stream.recv()).await
        });
        handle.try_send_resize(106, 32).expect("resize");
        std::thread::sleep(Duration::from_millis(60));
        handle
            .try_send_user(b"stty size; printf '%s%s\\n' FISH_COMPAT_ OK; exit 0\n".to_vec())
            .expect("send fish cmd");

        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !out.contains("FISH_COMPAT_OK") {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), stream.recv()).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Err(_) => {}
            }
        }
        guard.stop_blocking();
        assert!(out.contains("32 106"), "fish stty size mismatch: {out:?}");
        assert!(out.contains("FISH_COMPAT_OK"), "fish failed: {out:?}");
    }

    // 2. Neovim alternate screen + resize check (non-interactive headless script in real PTY)
    if has_cmd("nvim") {
        let qa_home = format!("/tmp/u-gp-itnv-{}", std::process::id());
        let _ = std::fs::create_dir_all(&qa_home);
        let mut env = os::build_child_shell_env(Some("/bin/sh"));
        env.insert("HOME".into(), qa_home.clone());
        env.insert("XDG_CONFIG_HOME".into(), format!("{qa_home}/cfg"));
        env.insert("XDG_STATE_HOME".into(), format!("{qa_home}/st"));
        env.insert("XDG_DATA_HOME".into(), format!("{qa_home}/data"));

        let opts = HerdrStartOptions {
            tag: "nvim".into(),
            argv: Some(vec![
                "nvim".into(),
                "--clean".into(),
                "-c".into(),
                "echo 'NVIM_' . 'BOOT_OK'".into(),
            ]),
            cwd: None,
            env,
            cols: 80,
            rows: 24,
            history_lines: 200,
            wrap_iutf8: false,
        };
        let (mut guard, handle, mut stream) = HerdrGuard::start_live(opts).expect("start nvim");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut raw_bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), stream.recv()).await
            }) {
                raw_bytes.extend_from_slice(&bytes);
                if raw_bytes.windows(b"NVIM_BOOT_OK".len()).any(|w| w == b"NVIM_BOOT_OK") {
                    break;
                }
            }
        }
        // Wait for Neovim TUI startup bursts to settle, then deliver FRAME_RESIZE + Mode 2048 report
        // just like LiveTerminalView::sync_resize_if_needed.
        std::thread::sleep(Duration::from_millis(80));
        while let Some(SessionOutput::Data(bytes)) = stream.try_recv() {
            raw_bytes.extend_from_slice(&bytes);
        }
        handle.try_send_resize(106, 32).expect("resize nvim");
        if raw_bytes.windows(b"\x1b[?2048h".len()).any(|w| w == b"\x1b[?2048h") {
            let _ = handle.try_send_automatic(b"\x1b[48;32;106;576;848t".to_vec());
        }
        let mut saw_resize = false;
        let resize_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < resize_deadline {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(100), stream.recv()).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    raw_bytes.extend_from_slice(&bytes);
                    saw_resize = true;
                }
                _ if saw_resize => break,
                _ => {}
            }
        }
        handle
            .try_send_user(b":echo &columns . 'x' . &lines\r".to_vec())
            .expect("query nvim size");
        std::thread::sleep(Duration::from_millis(120));
        handle.try_send_user(b":q!\r".to_vec()).expect("quit nvim");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), stream.recv()).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => raw_bytes.extend_from_slice(&bytes),
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Err(_) => {}
            }
        }
        guard.stop_blocking();
        let _ = std::fs::remove_dir_all(&qa_home);

        let text = String::from_utf8_lossy(&raw_bytes);
        assert!(
            raw_bytes.windows(b"\x1b[?1049h".len()).any(|w| w == b"\x1b[?1049h"),
            "expected alt-screen enter sequence from nvim"
        );
        assert!(
            raw_bytes.windows(b"\x1b[?1049l".len()).any(|w| w == b"\x1b[?1049l"),
            "expected alt-screen exit sequence from nvim"
        );
        assert!(
            text.contains("106x32"),
            "expected resized 106x32 from nvim, got: {text:?}"
        );
    }

    // 3. Codex CLI smoke check in isolated herdr PTY
    if has_cmd("codex") {
        let opts = HerdrStartOptions {
            tag: "codex".into(),
            argv: Some(vec!["codex".into(), "--version".into()]),
            cwd: None,
            env: os::build_child_shell_env(Some("/bin/sh")),
            cols: 80,
            rows: 24,
            history_lines: 200,
            wrap_iutf8: false,
        };
        let (mut guard, _handle, mut stream) = HerdrGuard::start_live(opts).expect("start codex");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), stream.recv()).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Err(_) => {}
            }
        }
        guard.stop_blocking();
        assert!(
            out.to_ascii_lowercase().contains("codex"),
            "expected codex version output, got: {out:?}"
        );
    }
}
