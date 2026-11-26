//! Phase 0 Backend Integration Tests against real isolated herdr-core v0.1.22
//! and external Node.js 24 TypeScript config runtime.

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::timeout;
use uiterm_lib::config_runtime::{ConfigEvalParams, eval_config_poc};
use uiterm_lib::herdr::{
    CoreEvent, HerdrClient, HerdrDaemon, HerdrLaunchConfig, OutputStreamFrame,
    TerminalAttachParams, TerminalCreateParams,
};

async fn collect_until(
    rx: &mut tokio::sync::mpsc::Receiver<OutputStreamFrame>,
    client: &HerdrClient,
    attach_id: u32,
    needle: &str,
    max_wait: Duration,
) -> String {
    let mut acc = String::new();
    let deadline = tokio::time::Instant::now() + max_wait;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(remaining, rx.recv()).await {
            Ok(Some(OutputStreamFrame::Data(bytes))) => {
                let len = bytes.len() as u32;
                acc.push_str(&String::from_utf8_lossy(&bytes));
                let _ = client.terminal_ack(attach_id, len).await;
                if acc.contains(needle) {
                    return acc;
                }
            }
            Ok(Some(OutputStreamFrame::Resize { .. })) => {}
            Ok(Some(OutputStreamFrame::Close(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }
    acc
}

#[tokio::test]
async fn herdr_core_lifecycle_io_resize_auto_input_and_osc7501() {
    let cfg = HerdrLaunchConfig::isolated("it1").expect("isolated config");
    let mut daemon = HerdrDaemon::start(cfg.clone()).await.expect("start daemon");
    let client = HerdrClient::connect(daemon.socket_path(), &cfg.session)
        .await
        .expect("connect framed");

    let status = client.server_status().await.expect("server.status");
    assert!(status.pid > 0);
    assert!(
        status.automatic_input_supported,
        "herdr-core 0.1.22 must advertise automatic_input"
    );
    assert!(
        status.resize_notifications_supported,
        "herdr-core 0.1.22 must advertise resize_notifications"
    );

    // Subscribe to events before creating terminals.
    let (sub, mut ev_rx) = client.events_subscribe().await.expect("events.subscribe");
    assert_eq!(sub.terminals.len(), 0);

    // Create interactive sh session.
    let term = client
        .terminal_create(TerminalCreateParams {
            argv: Some(vec!["/bin/sh".into()]),
            cols: Some(80),
            rows: Some(24),
            name: Some("it-shell".into()),
            ..Default::default()
        })
        .await
        .expect("terminal.run");
    assert_eq!(term.state, "live");

    // Attach to session.
    let mut attach = client
        .terminal_attach(TerminalAttachParams {
            session_id: term.address.clone(),
            cols: 80,
            rows: 24,
            history: Some(100),
            observe: Some(false),
            resize_notifications: Some(true),
            display: None,
        })
        .await
        .expect("terminal.attach");
    assert!(attach.result.automatic_input);
    let attach_id = attach.result.attach_id;

    // 0. Defensive check: attachId=0 must be rejected without touching JSON-RPC channel 0
    let zero_ack_err = client.terminal_ack(0, 1024).await.unwrap_err();
    assert_eq!(zero_ack_err.code, "invalid_args");
    let zero_write_err = client
        .terminal_write(0, b"x".to_vec(), true)
        .await
        .unwrap_err();
    assert_eq!(zero_write_err.code, "invalid_args");

    // 1. User input (DATA frame kind 0)
    client
        .terminal_write(attach_id, b"echo UITERM_USER_OK\n".to_vec(), false)
        .await
        .expect("write user");
    let out = collect_until(
        &mut attach.rx,
        &client,
        attach_id,
        "UITERM_USER_OK",
        Duration::from_secs(5),
    )
    .await;
    assert!(out.contains("UITERM_USER_OK"), "got output: {out:?}");

    // 2. Resize (RESIZE frame kind 3) -> verify via `stty size`
    client
        .terminal_resize(attach_id, 111, 33)
        .await
        .expect("resize");
    tokio::time::sleep(Duration::from_millis(100)).await;
    client
        .terminal_write(attach_id, b"stty size\n".to_vec(), false)
        .await
        .expect("write stty");
    let out = collect_until(
        &mut attach.rx,
        &client,
        attach_id,
        "33 111",
        Duration::from_secs(5),
    )
    .await;
    assert!(
        out.contains("33 111"),
        "expected resized 33 111, got: {out:?}"
    );

    // 3. Automatic input (AUTOMATIC_INPUT frame kind 6) -> verify bytes reach PTY unchanged
    client
        .terminal_write(attach_id, b"echo UITERM_AUTO_OK\n".to_vec(), true)
        .await
        .expect("write automatic_input");
    let out = collect_until(
        &mut attach.rx,
        &client,
        attach_id,
        "UITERM_AUTO_OK",
        Duration::from_secs(5),
    )
    .await;
    assert!(out.contains("UITERM_AUTO_OK"), "got output: {out:?}");

    // 4. OSC 7501 agent status reporting -> verify CoreEvent::Status and Turn arrive
    client
        .terminal_write(
            attach_id,
            b"printf '\\033]7501;state=working:app=codex\\007'\n".to_vec(),
            false,
        )
        .await
        .expect("write osc7501 working");

    let mut saw_working = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Some(batch)) = timeout(rem, ev_rx.recv()).await {
            if batch.bytes > 0 {
                client
                    .events_ack(sub.subscription_id, batch.bytes)
                    .await
                    .unwrap();
            }
            if batch.events.into_iter().any(|ev| {
                matches!(ev, CoreEvent::Status { terminal, data, .. }
                    if terminal == term.address && data["status"]["state"] == "working")
            }) {
                saw_working = true;
                break;
            }
        } else {
            break;
        }
    }
    assert!(saw_working, "expected OSC 7501 status.state=working event");

    client
        .terminal_write(
            attach_id,
            b"printf '\\033]7501;state=done:app=codex\\007'\n".to_vec(),
            false,
        )
        .await
        .expect("write osc7501 done");

    let mut saw_done = false;
    let mut saw_turn = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_done && saw_turn) {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(Some(batch)) = timeout(rem, ev_rx.recv()).await {
            if batch.bytes > 0 {
                client
                    .events_ack(sub.subscription_id, batch.bytes)
                    .await
                    .unwrap();
            }
            for ev in batch.events {
                match ev {
                    CoreEvent::Status { terminal, data, .. }
                        if terminal == term.address
                            && data
                                .get("status")
                                .and_then(|s| s.get("state"))
                                .and_then(|v| v.as_str())
                                == Some("done") =>
                    {
                        saw_done = true;
                    }
                    CoreEvent::Turn { terminal, data, .. }
                        if terminal == term.address
                            && data.get("turn").and_then(|v| v.as_u64()).unwrap_or(0) >= 1 =>
                    {
                        saw_turn = true;
                    }
                    _ => {}
                }
            }
        } else {
            break;
        }
    }
    assert!(saw_done, "expected OSC 7501 status.state=done event");
    assert!(
        saw_turn,
        "expected OSC 7501 turn>=1 event after leaving working"
    );

    // 5. Detach and re-attach (resync repaint)
    client.terminal_detach(attach_id).await.expect("detach");
    let mut reattach = client
        .terminal_attach(TerminalAttachParams {
            session_id: term.address.clone(),
            cols: 111,
            rows: 33,
            history: Some(100),
            observe: Some(false),
            resize_notifications: Some(true),
            display: None,
        })
        .await
        .expect("reattach");
    let repaint = collect_until(
        &mut reattach.rx,
        &client,
        reattach.result.attach_id,
        "UITERM_AUTO_OK",
        Duration::from_secs(3),
    )
    .await;
    assert!(
        repaint.contains("UITERM_AUTO_OK"),
        "expected repaint to contain previous screen output, got: {repaint:?}"
    );

    // 6. Kill and verify cleanup
    client
        .terminal_kill(&term.address)
        .await
        .expect("terminal.kill");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let live = client.terminal_list(false).await.expect("terminal.list");
    assert!(live.is_empty(), "expected 0 live terminals after kill");
    let all = client.terminal_list(true).await.expect("list including killed terminal");
    let finished = all.iter().find(|item| item.address == term.address).unwrap();
    assert_eq!(finished.state, "finished");
    assert!(finished.signal.is_some(), "signal metadata must survive the IPC boundary");

    client
        .events_unsubscribe(sub.subscription_id)
        .await
        .expect("unsubscribe");
    daemon.stop().await.expect("stop daemon");
}

#[tokio::test]
async fn config_runtime_eval_ts_actions_and_hooks() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sample = manifest_dir
        .parent()
        .unwrap()
        .join("poc/config-runtime/sample.config.ts");

    let res = eval_config_poc(ConfigEvalParams {
        config_path: sample.to_string_lossy().to_string(),
        action_name: Some("greet".into()),
        action_args: Some(serde_json::json!({ "name": "uiterm" })),
        event_name: Some("onAgentStatus".into()),
        event_payload: Some(serde_json::json!({
            "terminal": "t1",
            "status": "blocked",
            "turn": 3
        })),
    })
    .await
    .expect("eval_config_poc");

    assert_eq!(res.config["theme"], "uiterm-dark");
    assert_eq!(res.action_result["message"], "こんにちは、uitermさん");
    assert_eq!(res.hook_result["notified"], true);
    assert!(res.startup_ms > 0.0);
    assert!(res.worker_rss_bytes.unwrap_or(0) > 0);
}

#[tokio::test]
async fn stalled_pty_window_preserves_other_streams_and_control() {
    use uiterm_lib::herdr::protocol::INITIAL_CREDIT_BYTES;

    let cfg = HerdrLaunchConfig::isolated("credit").unwrap();
    let mut daemon = HerdrDaemon::start(cfg).await.unwrap();
    let client = HerdrClient::connect(daemon.socket_path(), &daemon.config().session)
        .await
        .unwrap();
    let mut attachments = Vec::new();
    for _ in 0..2 {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec!["/bin/sh".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        let attachment = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address,
                cols: 80,
                rows: 24,
                history: None,
                observe: None,
                resize_notifications: None,
                display: None,
            })
            .await
            .unwrap();
        attachments.push(attachment);
    }
    let mut fast = attachments.pop().unwrap();
    let mut slow = attachments.pop().unwrap();
    let slow_id = slow.result.attach_id;
    client
        .terminal_write(
            slow_id,
            b"yes uiterm | head -c 524288; printf 'CREDIT_DONE\\n'\n".to_vec(),
            false,
        )
        .await
        .unwrap();

    let mut consumed = 0;
    timeout(Duration::from_secs(5), async {
        while consumed < INITIAL_CREDIT_BYTES {
            match slow.rx.recv().await.unwrap() {
                OutputStreamFrame::Data(bytes) => consumed += bytes.len() as u32,
                OutputStreamFrame::Resize { .. } => {}
                OutputStreamFrame::Close(_) => {
                    panic!("slow stream closed before credit was restored")
                }
            }
        }
    })
    .await
    .expect("fill the initial credit window");
    assert_eq!(consumed, INITIAL_CREDIT_BYTES);
    assert!(
        timeout(Duration::from_millis(100), slow.rx.recv())
            .await
            .is_err()
    );
    assert_eq!(
        client
            .terminal_ack(slow_id, consumed + 1)
            .await
            .unwrap_err()
            .code,
        "invalid_args"
    );

    assert_eq!(
        timeout(Duration::from_secs(3), client.server_status())
            .await
            .unwrap()
            .unwrap()
            .live,
        2
    );
    let fast_id = fast.result.attach_id;
    client
        .terminal_write(fast_id, b"printf 'FAST_%s\\n' OK\n".to_vec(), false)
        .await
        .unwrap();
    let fast_output = collect_until(
        &mut fast.rx,
        &client,
        fast_id,
        "FAST_OK",
        Duration::from_secs(3),
    )
    .await;
    assert!(fast_output.contains("FAST_OK"));

    client.terminal_ack(slow_id, consumed).await.unwrap();
    let slow_output = collect_until(
        &mut slow.rx,
        &client,
        slow_id,
        "CREDIT_DONE",
        Duration::from_secs(5),
    )
    .await;
    assert!(slow_output.contains("CREDIT_DONE"));
    let socket = daemon.socket_path().to_owned();
    daemon.stop().await.unwrap();
    assert!(!socket.parent().unwrap().exists());
}

#[tokio::test]
async fn pty_preserves_japanese_bytes_with_canonical_utf8_erase() {
    let cfg = HerdrLaunchConfig::isolated("utf8").unwrap();
    let mut daemon = HerdrDaemon::start(cfg).await.unwrap();
    let client = HerdrClient::connect(daemon.socket_path(), &daemon.config().session)
        .await
        .unwrap();
    for (iutf8, expected) in [
        ("-iutf8", " 41 e6 97 a5 e6 9c 42"),
        ("iutf8", " 41 e6 97 a5 42"),
    ] {
        let term = client.terminal_create(TerminalCreateParams {
            argv: Some(vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("stty -a; stty icanon -echo erase '^?' {iutf8} || exit; printf 'PTY_READY\\n'; IFS= read -r line; printf '%s' \"$line\" | od -An -tx1"),
            ]),
            ..Default::default()
        }).await.unwrap();
        let mut attached = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address,
                cols: 120,
                rows: 40,
                history: None,
                observe: None,
                resize_notifications: None,
                display: None,
            })
            .await
            .unwrap();
        let id = attached.result.attach_id;
        let ready = collect_until(
            &mut attached.rx,
            &client,
            id,
            "PTY_READY",
            Duration::from_secs(3),
        )
        .await;
        assert!(ready.contains("PTY_READY"), "{ready:?}");
        #[cfg(target_os = "macos")]
        assert!(
            ready.contains("-iutf8"),
            "expected pinned macOS core's initial termios: {ready:?}"
        );
        client
            .terminal_write(id, "A日本\u{7f}B\n".as_bytes().to_vec(), false)
            .await
            .unwrap();
        let output = collect_until(
            &mut attached.rx,
            &client,
            id,
            expected,
            Duration::from_secs(3),
        )
        .await;
        assert!(
            output.contains(expected),
            "{iutf8}: wanted {expected:?}, got {output:?}"
        );
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn pty_iutf8_init_enables_utf8_erase_on_fresh_herdr_pty() {
    let cfg = HerdrLaunchConfig::isolated("iutf8-init").unwrap();
    let mut daemon = HerdrDaemon::start(cfg).await.unwrap();
    let client = HerdrClient::connect(daemon.socket_path(), &daemon.config().session)
        .await
        .unwrap();
    let uiterm_bin = env!("CARGO_BIN_EXE_uiterm");

    for (input, expected_hex) in [
        // Reported repro: "あいうえお" + 5x Backspace (DEL) + "かきくけこ" -> valid UTF-8 "かきくけこ"
        (
            "あいうえお\u{7f}\u{7f}\u{7f}\u{7f}\u{7f}かきくけこ\n",
            " e3 81 8b e3 81 8d e3 81 8f e3 81 91 e3 81 93",
        ),
        // Mixed ASCII + Kanji: "A日本" + DEL + "B" -> "A日B"
        ("A日本\u{7f}B\n", " 41 e6 97 a5 42"),
    ] {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec![
                    uiterm_bin.into(),
                    uiterm_lib::os::PTY_IUTF8_INIT_FLAG.into(),
                    "--".into(),
                    "/bin/sh".into(),
                    "-c".into(),
                    // Deliberately do NOT run `stty iutf8`; rely on `--uiterm-pty-init`.
                    "stty -a; stty icanon -echo erase '^?' || exit; printf 'PTY_READY\\n'; IFS= read -r line; printf '%s' \"$line\" | od -An -tx1".into(),
                ]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            term.argv.first().map(String::as_str),
            Some("/bin/sh"),
            "TerminalInfo.argv must strip the internal PTY init wrapper: {:?}",
            term.argv
        );
        let mut attached = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address,
                cols: 120,
                rows: 40,
                history: None,
                observe: None,
                resize_notifications: None,
                display: None,
            })
            .await
            .unwrap();
        let id = attached.result.attach_id;
        let ready = collect_until(
            &mut attached.rx,
            &client,
            id,
            "PTY_READY",
            Duration::from_secs(3),
        )
        .await;
        assert!(ready.contains("PTY_READY"), "{ready:?}");
        assert!(
            !ready.contains("-iutf8") && ready.contains("iutf8"),
            "fresh PTY must have iutf8 enabled without stty iutf8: {ready:?}"
        );
        client
            .terminal_write(id, input.as_bytes().to_vec(), false)
            .await
            .unwrap();
        let output = collect_until(
            &mut attached.rx,
            &client,
            id,
            expected_hex,
            Duration::from_secs(3),
        )
        .await;
        assert!(
            output.contains(expected_hex),
            "wanted {expected_hex:?}, got {output:?}"
        );
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires local fish, nvim and codex executables"]
async fn installed_cli_compatibility_in_isolated_pty() {
    let cfg = HerdrLaunchConfig::isolated("cli").unwrap();
    let mut daemon = HerdrDaemon::start(cfg).await.unwrap();
    let client = HerdrClient::connect(daemon.socket_path(), &daemon.config().session)
        .await
        .unwrap();

    for (argv, keys, output) in [
        (
            vec!["fish", "--no-config", "--private", "-i"],
            "printf '日本語%s\\n' FISH_OK\rexit\r",
            "日本語FISH_OK",
        ),
        (
            vec!["nvim", "--clean", "-n", "-i", "NONE"],
            "i日本語NVIM_OK",
            "日本語NVIM_OK",
        ),
        (vec!["codex", "--version"], "", "codex-cli "),
    ] {
        // Gate the short-lived programs on a line from the viewer, so attach does not race exit.
        let mut command = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "read -r _; exec \"$@\"".to_owned(),
            "sh".to_owned(),
        ];
        command.extend(argv.iter().map(|part| (*part).to_owned()));
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(command),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut attach = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address,
                cols: 100,
                rows: 35,
                history: None,
                observe: None,
                resize_notifications: None,
                display: None,
            })
            .await
            .unwrap();
        let id = attach.result.attach_id;
        client
            .terminal_write(id, b"begin\n".to_vec(), false)
            .await
            .unwrap();
        if !keys.is_empty() {
            let ready = if argv[0] == "nvim" {
                "\x1b[?1049h"
            } else {
                ">"
            };
            let paint =
                collect_until(&mut attach.rx, &client, id, ready, Duration::from_secs(3)).await;
            assert!(paint.contains(ready), "{}: {paint:?}", argv[0]);
            client
                .terminal_write(id, keys.as_bytes().to_vec(), false)
                .await
                .unwrap();
        }

        let mut text = Vec::new();
        if argv[0] == "nvim" {
            let paint =
                collect_until(&mut attach.rx, &client, id, output, Duration::from_secs(3)).await;
            text.extend(paint.into_bytes());
            client
                .terminal_write(id, b"\x1c\x0e:q!\r".to_vec(), false)
                .await
                .unwrap();
        }
        timeout(Duration::from_secs(10), async {
            loop {
                match attach.rx.recv().await.unwrap() {
                    OutputStreamFrame::Data(bytes) => {
                        let _ = client.terminal_ack(id, bytes.len() as u32).await;
                        text.extend(bytes);
                    }
                    OutputStreamFrame::Resize { .. } => {}
                    OutputStreamFrame::Close(details) => {
                        let close: serde_json::Value = serde_json::from_slice(&details).unwrap();
                        assert_eq!(close["exit_code"], 0, "{}: {close}", argv[0]);
                        break;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{} did not exit after receiving {} output bytes",
                argv[0],
                text.len()
            )
        });
        let text = String::from_utf8_lossy(&text);
        assert!(
            text.contains(output),
            "{} did not display {output:?}: {text:?}",
            argv[0]
        );
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reconnect_repaints_the_live_session_and_rejects_old_attach_id() {
    let cfg = HerdrLaunchConfig::isolated("reconnect").unwrap();
    let mut daemon = HerdrDaemon::start(cfg).await.unwrap();
    let connect = || HerdrClient::connect(daemon.socket_path(), &daemon.config().session);
    let client = connect().await.unwrap();
    let initial_status = client.server_status().await.unwrap();
    let terminal = client
        .terminal_create(TerminalCreateParams {
            argv: Some(vec!["/bin/sh".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
    let params = TerminalAttachParams {
        session_id: terminal.address,
        cols: 80,
        rows: 24,
        history: None,
        observe: None,
        resize_notifications: None,
        display: None,
    };
    let mut old = client.terminal_attach(params.clone()).await.unwrap();
    let old_id = old.result.attach_id;
    client
        .terminal_write(old_id, b"printf 'RECONNECT_%s\\n' TEST\n".to_vec(), false)
        .await
        .unwrap();
    let paint = collect_until(
        &mut old.rx,
        &client,
        old_id,
        "RECONNECT_TEST",
        Duration::from_secs(3),
    )
    .await;
    assert!(paint.contains("RECONNECT_TEST"));

    drop(client);
    timeout(Duration::from_secs(3), async {
        while let Some(frame) = old.rx.recv().await {
            if let OutputStreamFrame::Close(bytes) = frame {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["reason"],
                    "disconnected"
                );
                return;
            }
        }
        panic!("stream ended without CLOSE");
    })
    .await
    .unwrap();

    let client = connect().await.unwrap();
    assert_eq!(
        client.server_status().await.unwrap().pid,
        initial_status.pid
    );
    let mut new = client.terminal_attach(params).await.unwrap();
    let new_id = new.result.attach_id;
    assert_ne!(new_id, old_id);
    assert_eq!(
        client
            .terminal_write(old_id, b"stale".to_vec(), false)
            .await
            .unwrap_err()
            .code,
        "invalid_attach"
    );
    let paint = collect_until(
        &mut new.rx,
        &client,
        new_id,
        "RECONNECT_TEST",
        Duration::from_secs(3),
    )
    .await;
    assert!(paint.contains("RECONNECT_TEST"));
    daemon.stop().await.unwrap();
}
