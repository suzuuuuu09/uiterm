//! Integration & compatibility tests against real isolated `herdr-core` v0.1.22,
//! `uiterm --uiterm-pty-init`, `fish`, `nvim`, and `codex --version`.

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../src/os.rs"]
mod os;

#[allow(dead_code)]
#[allow(unused_imports)]
#[path = "../src/herdr.rs"]
mod herdr;

#[allow(dead_code)]
#[path = "../src/vt.rs"]
mod vt;

use herdr::{HerdrGuard, HerdrOutputStream, HerdrStartOptions, SessionOutput};

async fn recv_output(stream: &mut HerdrOutputStream) -> Option<SessionOutput> {
    let event = stream.recv_uncredited().await?;
    if let SessionOutput::Data(bytes) = &event {
        // A closed writer can still have final output queued for the test to read.
        let _ = stream.acknowledge(bytes.len() as u32, Vec::new()).await;
    }
    Some(event)
}

fn has_cmd(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn private_core_pid(path: &std::path::Path) -> i32 {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(
        socket,
        r#"{{"jsonrpc":"2.0","id":1,"method":"server.status","params":{{}}}}"#
    )
    .unwrap();
    let mut reply = String::new();
    BufReader::new(socket).read_line(&mut reply).unwrap();
    let response: serde_json::Value = serde_json::from_str(&reply).unwrap();
    i32::try_from(response["result"]["pid"].as_u64().unwrap()).unwrap()
}

#[test]
fn reconnect_replays_the_same_pty_after_core_crash() {
    let opts = HerdrStartOptions {
        argv: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s\\n' INITIAL_SCREEN; read value; printf 'REATTACHED:%s\\n' \"$value\""
                .into(),
        ]),
        wrap_iutf8: false,
        ..HerdrStartOptions::default()
    };
    let (mut guard, handle, mut stream) =
        HerdrGuard::start_with_executable(opts, std::path::Path::new(env!("CARGO_BIN_EXE_uiterm")))
            .expect("start herdr");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let collect_until = |stream: &mut herdr::HerdrOutputStream, marker: &[u8]| {
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                let mut output = Vec::new();
                while let Some(event) = recv_output(stream).await {
                    if let SessionOutput::Data(bytes) = event {
                        output.extend(bytes);
                        if output.windows(marker.len()).any(|part| part == marker) {
                            return;
                        }
                    }
                }
                panic!("stream ended before the expected terminal output");
            })
            .await
            .expect("terminal output arrived");
        });
    };
    collect_until(&mut stream, b"INITIAL_SCREEN");
    let previous_handle = handle;
    let mut previous_stream = stream;
    let (handle, mut stream) = guard
        .reattach(HerdrStartOptions::default())
        .expect("reattach");
    assert!(
        previous_handle
            .try_send_user(b"old connection".to_vec())
            .is_err()
    );
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while previous_stream.recv_uncredited().await.is_some() {}
        })
        .await
        .expect("old attachment reached EOF");
    });
    collect_until(&mut stream, b"INITIAL_SCREEN");

    let socket = guard.socket_path().to_path_buf();
    let core_pid = private_core_pid(&socket);
    assert!(core_pid > 0);
    // The PID comes from the supervisor's private socket for this test instance.
    assert_eq!(unsafe { libc::kill(core_pid, libc::SIGKILL) }, 0);
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = recv_output(&mut stream).await {
                if let SessionOutput::Closed(info) = event {
                    assert_eq!(info.reason, "disconnected");
                    return;
                }
            }
            panic!("the old attachment ended without a disconnect event");
        })
        .await
        .unwrap();
    });
    assert!(handle.try_send_user(b"stale attachment".to_vec()).is_err());
    let (handle, mut stream) = guard
        .reattach(HerdrStartOptions::default())
        .expect("reattach after core recovery");
    assert_ne!(private_core_pid(&socket), core_pid);
    collect_until(&mut stream, b"INITIAL_SCREEN");
    handle
        .try_send_user(b"same command\n".to_vec())
        .expect("write to original PTY");
    collect_until(&mut stream, b"REATTACHED:same command");
    guard.stop_blocking();
    assert!(!socket.exists());
}

#[test]
fn pty_iutf8_init_enables_utf8_backspace_on_real_herdr_pty() {
    let bin = env!("CARGO_BIN_EXE_uiterm");
    let wrapped = os::wrap_argv_with_pty_iutf8_init(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "stty -echo && stty -a && printf '%s\\n' IUTF8_READY && exec cat".into(),
        ],
        bin,
    );

    let opts = HerdrStartOptions {
        argv: Some(wrapped),
        cwd: None,
        env: HashMap::from([("LANG".into(), "en_US.UTF-8".into())]),
        cols: 80,
        rows: 24,
        history_lines: 200,
        wrap_iutf8: false,
    };

    let (mut guard, handle, mut stream) =
        HerdrGuard::start_with_executable(opts, std::path::Path::new(env!("CARGO_BIN_EXE_uiterm")))
            .expect("start herdr");
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut startup = Vec::new();
    let ready = b"IUTF8_READY\r\n";
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !startup.ends_with(ready) {
        if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(200), recv_output(&mut stream)).await
        }) {
            startup.extend(bytes);
        }
    }
    assert!(startup.ends_with(ready), "expected PTY setup to finish");
    let settings = String::from_utf8_lossy(&startup);
    assert!(settings.split_whitespace().any(|word| word == "iutf8"));
    assert!(settings.split_whitespace().any(|word| word == "-echo"));

    // Canonical Japanese Backspace test: "あいうえお" + 5x DEL + "かきくけこ\n" -> "かきくけこ"
    let mut seq = "あいうえお".as_bytes().to_vec();
    seq.extend_from_slice(&[0x7f; 5]);
    seq.extend_from_slice("かきくけこ\n".as_bytes());
    seq.extend_from_slice("A日本".as_bytes());
    seq.push(0x7f);
    seq.extend_from_slice("B\n".as_bytes());
    seq.push(0x04); // Ctrl+D EOF
    handle.try_send_user(seq).expect("send canonical input");

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(250), recv_output(&mut stream)).await
        }) {
            Ok(Some(SessionOutput::Data(bytes))) => {
                output.extend(bytes);
            }
            Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
            Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
        }
    }
    guard.stop_blocking();

    assert_eq!(output, "かきくけこ\r\nA日B\r\n".as_bytes());
}

#[test]
#[ignore = "requires fish, nvim, and codex on PATH"]
fn installed_clis_fish_nvim_codex_and_resize_altscreen_work_in_isolated_pty() {
    for cli in ["fish", "nvim", "codex"] {
        assert!(
            has_cmd(cli),
            "install {cli} before running CLI compatibility checks"
        );
    }
    let bin = env!("CARGO_BIN_EXE_uiterm");

    // 1. Fish shell + resize check
    {
        let wrapped = os::wrap_argv_with_pty_iutf8_init(
            vec!["fish".into(), "-l".into(), "--no-config".into()],
            bin,
        );
        let opts = HerdrStartOptions {
            argv: Some(wrapped),
            cwd: None,
            env: os::build_child_shell_env(Some("fish")),
            cols: 80,
            rows: 24,
            history_lines: 200,
            wrap_iutf8: false,
        };
        let (mut guard, handle, mut stream) = HerdrGuard::start_with_executable(
            opts,
            std::path::Path::new(env!("CARGO_BIN_EXE_uiterm")),
        )
        .expect("start fish");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(300), recv_output(&mut stream)).await
        });
        assert!(
            handle
                .try_update_size_with_reports(Some((106, 32)), || Ok(Vec::new()))
                .unwrap()
        );
        std::thread::sleep(Duration::from_millis(60));
        handle
            .try_send_user(b"stty size; printf '%s%s\\n' FISH_COMPAT_ OK; exit 0\n".to_vec())
            .expect("send fish cmd");

        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !out.contains("FISH_COMPAT_OK") {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), recv_output(&mut stream)).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        guard.stop_blocking();
        assert!(out.contains("32 106"), "fish stty size mismatch: {out:?}");
        assert!(out.contains("FISH_COMPAT_OK"), "fish failed: {out:?}");
    }

    // 2. Neovim alternate screen + resize check (non-interactive headless script in real PTY)
    {
        let qa_home = format!("/tmp/u-gp-itnv-{}", std::process::id());
        let _ = std::fs::create_dir_all(&qa_home);
        let mut env = os::build_child_shell_env(Some("/bin/sh"));
        env.insert("HOME".into(), qa_home.clone());
        env.insert("XDG_CONFIG_HOME".into(), format!("{qa_home}/cfg"));
        env.insert("XDG_STATE_HOME".into(), format!("{qa_home}/st"));
        env.insert("XDG_DATA_HOME".into(), format!("{qa_home}/data"));

        let opts = HerdrStartOptions {
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
        let (mut guard, handle, mut stream) = HerdrGuard::start_with_executable(
            opts,
            std::path::Path::new(env!("CARGO_BIN_EXE_uiterm")),
        )
        .expect("start nvim");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut raw_bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), recv_output(&mut stream)).await
            }) {
                raw_bytes.extend_from_slice(&bytes);
                if raw_bytes
                    .windows(b"NVIM_BOOT_OK".len())
                    .any(|w| w == b"NVIM_BOOT_OK")
                {
                    break;
                }
            }
        }
        // Wait for Neovim TUI startup bursts to settle, then deliver FRAME_RESIZE + Mode 2048 report
        // just like LiveTerminalView::sync_resize_if_needed.
        std::thread::sleep(Duration::from_millis(80));
        while let Some(SessionOutput::Data(bytes)) = stream.try_recv_uncredited() {
            rt.block_on(async { stream.acknowledge(bytes.len() as u32, Vec::new()).await })
                .expect("credit Neovim startup output");
            raw_bytes.extend_from_slice(&bytes);
        }
        let mut terminal = vt::VtTerminal::new(80, 24, 200).expect("VT terminal");
        let enter = b"\x1b[?1049h";
        let switch = raw_bytes
            .windows(enter.len())
            .position(|part| part == enter)
            .expect("Neovim enters the alternate screen");
        terminal.feed(&raw_bytes[..switch]);
        terminal.feed(b"\x1b[Hprimary screen");
        terminal.feed(&raw_bytes[switch..]);
        assert_eq!(
            terminal.active_screen(),
            ghostty_vt::screen::Screen::Alternate
        );
        assert!(
            handle
                .try_update_size_with_reports(Some((106, 32)), || terminal.resize(106, 32, 8, 18))
                .unwrap()
        );
        let resized = terminal.snapshot().expect("alternate screen snapshot");
        assert_eq!((resized.cols, resized.rows), (106, 32));
        let replayed = raw_bytes.len();
        let mut saw_resize = false;
        let resize_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < resize_deadline {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(100), recv_output(&mut stream)).await
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
                tokio::time::timeout(Duration::from_millis(200), recv_output(&mut stream)).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => raw_bytes.extend_from_slice(&bytes),
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        guard.stop_blocking();
        let _ = std::fs::remove_dir_all(&qa_home);

        let tail = &raw_bytes[replayed..];
        let leave = b"\x1b[?1049l";
        let switch = tail
            .windows(leave.len())
            .position(|part| part == leave)
            .expect("Neovim leaves the alternate screen");
        terminal.feed(&tail[..switch + leave.len()]);
        assert_eq!(
            terminal.active_screen(),
            ghostty_vt::screen::Screen::Primary
        );
        let primary = terminal.snapshot().expect("restored primary screen");
        assert!(primary.lines.iter().any(|row| {
            let text: String = row.iter().map(|cell| cell.text.as_str()).collect();
            text.contains("primary screen")
        }));

        let text = String::from_utf8_lossy(&raw_bytes);
        assert!(
            raw_bytes
                .windows(b"\x1b[?1049h".len())
                .any(|w| w == b"\x1b[?1049h"),
            "expected alt-screen enter sequence from nvim"
        );
        assert!(
            raw_bytes
                .windows(b"\x1b[?1049l".len())
                .any(|w| w == b"\x1b[?1049l"),
            "expected alt-screen exit sequence from nvim"
        );
        assert!(
            text.contains("106x32"),
            "expected resized 106x32 from nvim, got: {text:?}"
        );
    }

    // 3. Codex CLI smoke check in isolated herdr PTY
    {
        let opts = HerdrStartOptions {
            argv: Some(vec!["codex".into(), "--version".into()]),
            cwd: None,
            env: os::build_child_shell_env(Some("/bin/sh")),
            cols: 80,
            rows: 24,
            history_lines: 200,
            wrap_iutf8: false,
        };
        let (mut guard, _handle, mut stream) = HerdrGuard::start_with_executable(
            opts,
            std::path::Path::new(env!("CARGO_BIN_EXE_uiterm")),
        )
        .expect("start codex");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(200), recv_output(&mut stream)).await
            }) {
                Ok(Some(SessionOutput::Data(bytes))) => {
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Some(SessionOutput::Closed(_))) | Ok(None) => break,
                Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
            }
        }
        guard.stop_blocking();
        assert!(
            out.to_ascii_lowercase().contains("codex"),
            "expected codex version output, got: {out:?}"
        );
    }
}
