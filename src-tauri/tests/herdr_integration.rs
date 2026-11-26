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
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<OutputStreamFrame>,
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
    let zero_write_err = client.terminal_write(0, b"x".to_vec(), true).await.unwrap_err();
    assert_eq!(zero_write_err.code, "invalid_args");

    // 1. User input (DATA frame kind 0)
    client
        .terminal_write(attach_id, b"echo UITERM_USER_OK\n".to_vec(), false)
        .await
        .expect("write user");
    let out = collect_until(&mut attach.rx, &client, attach_id, "UITERM_USER_OK", Duration::from_secs(5)).await;
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
    let out = collect_until(&mut attach.rx, &client, attach_id, "33 111", Duration::from_secs(5)).await;
    assert!(out.contains("33 111"), "expected resized 33 111, got: {out:?}");

    // 3. Automatic input (AUTOMATIC_INPUT frame kind 6) -> verify bytes reach PTY unchanged
    client
        .terminal_write(attach_id, b"echo UITERM_AUTO_OK\n".to_vec(), true)
        .await
        .expect("write automatic_input");
    let out = collect_until(&mut attach.rx, &client, attach_id, "UITERM_AUTO_OK", Duration::from_secs(5)).await;
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
        if let Ok(Some(ev)) = timeout(rem, ev_rx.recv()).await {
            if let CoreEvent::Status { terminal, data, .. } = ev
                && terminal == term.address
                && data
                    .get("status")
                    .and_then(|s| s.get("state"))
                    .and_then(|v| v.as_str())
                    == Some("working")
            {
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
        if let Ok(Some(ev)) = timeout(rem, ev_rx.recv()).await {
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
        } else {
            break;
        }
    }
    assert!(saw_done, "expected OSC 7501 status.state=done event");
    assert!(saw_turn, "expected OSC 7501 turn>=1 event after leaving working");

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
