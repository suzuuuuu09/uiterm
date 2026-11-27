//! Phase 0 Backend Verification & Benchmark Harness.
//!
//! Runs reproducible measurements against an isolated herdr-core instance:
//! 1. CLI/TUI compatibility: fish, nvim, codex
//! 2. Multi-session concurrency (1, 5, 10 terminals) + RSS memory
//! 3. High-throughput burst (5 MiB) + flow control / channel-0 isolation while stalled
//! 4. Session cleanup & orphan process verification
//! 5. External Node.js 24 TypeScript config runtime cold-start, RPC latency, RSS

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use uiterm_lib::config_runtime::{ConfigEvalParams, eval_config_poc};
use uiterm_lib::herdr::{
    HerdrClient, HerdrDaemon, HerdrLaunchConfig, OutputStreamFrame, TerminalAttachParams,
    TerminalCreateParams,
};

fn rss_kb_for_pids(pids: &[u32]) -> u64 {
    if pids.is_empty() {
        return 0;
    }
    let pid_list = pids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(out) = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid_list])
        .output()
    else {
        return 0;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .sum()
}

async fn wait_for_text(
    rx: &mut tokio::sync::mpsc::Receiver<OutputStreamFrame>,
    client: &HerdrClient,
    attach_id: u32,
    needle: &str,
    max_wait: Duration,
) -> (bool, usize) {
    let mut acc = String::new();
    let mut total_bytes = 0usize;
    let deadline = tokio::time::Instant::now() + max_wait;
    while tokio::time::Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(rem, rx.recv()).await {
            Ok(Some(OutputStreamFrame::Data(bytes))) => {
                total_bytes += bytes.len();
                let _ = client.terminal_ack(attach_id, bytes.len() as u32).await;
                acc.push_str(&String::from_utf8_lossy(&bytes));
                if acc.contains(needle) {
                    return (true, total_bytes);
                }
            }
            Ok(Some(OutputStreamFrame::Resize { .. })) => {}
            Ok(Some(OutputStreamFrame::Close(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }
    (false, total_bytes)
}

#[tokio::main]
async fn main() {
    println!("=== uiterm Phase 0 Backend Verification & Benchmark ===");

    let start_daemon = Instant::now();
    let cfg = HerdrLaunchConfig::isolated("bench").expect("isolated config");
    let mut daemon = HerdrDaemon::start(cfg.clone())
        .await
        .expect("start herdr-core");
    let daemon_start_ms = start_daemon.elapsed().as_secs_f64() * 1000.0;

    let connect_t0 = Instant::now();
    let client = HerdrClient::connect(daemon.socket_path(), &cfg.session)
        .await
        .expect("connect");
    let connect_ms = connect_t0.elapsed().as_secs_f64() * 1000.0;

    let status = client.server_status().await.expect("status");
    let status_val = client
        .rpc("server.status", serde_json::json!({}))
        .await
        .expect("raw status");
    let core_pid = status.pid;
    let idle_core_rss_kb = rss_kb_for_pids(&[core_pid]);

    println!(
        "[core] version={} pid={} daemon_start_ms={:.2} connect_ms={:.2} idle_rss_kb={} auto_input={} resize_notif={}",
        status.version,
        core_pid,
        daemon_start_ms,
        connect_ms,
        idle_core_rss_kb,
        status.automatic_input_supported,
        status.resize_notifications_supported
    );

    // 1. CLI/TUI compatibility: fish, nvim, codex
    println!("\n--- 1. CLI / TUI Compatibility ---");
    // fish
    {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec!["fish".into(), "-i".into()]),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            })
            .await
            .expect("fish create");
        let mut att = client
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
            .expect("fish attach");
        client
            .terminal_write(
                att.result.attach_id,
                b"echo FISH_OK_$FISH_VERSION\nexit\n".to_vec(),
                false,
            )
            .await
            .expect("fish write");
        let (ok, bytes) = wait_for_text(
            &mut att.rx,
            &client,
            att.result.attach_id,
            "FISH_OK_",
            Duration::from_secs(5),
        )
        .await;
        println!("[compat:fish] ok={ok} bytes={bytes}");
        let _ = client.terminal_kill(&term.address).await;
    }

    // nvim (interactive TUI in PTY, enter, :q)
    {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec!["nvim".into(), "--clean".into()]),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            })
            .await
            .expect("nvim create");
        let mut att = client
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
            .expect("nvim attach");
        // Wait briefly for alt-screen paint, then send :q!<CR>
        tokio::time::sleep(Duration::from_millis(400)).await;
        client
            .terminal_write(att.result.attach_id, b"\x1b:q!\r".to_vec(), false)
            .await
            .expect("nvim quit");
        let mut saw_close = false;
        let mut exit_code = None;
        let mut total_bytes = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(rem, att.rx.recv()).await {
                Ok(Some(OutputStreamFrame::Data(b))) => {
                    total_bytes += b.len();
                    let _ = client
                        .terminal_ack(att.result.attach_id, b.len() as u32)
                        .await;
                }
                Ok(Some(OutputStreamFrame::Close(payload))) => {
                    saw_close = true;
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        exit_code = v.get("exit_code").and_then(|c| c.as_i64());
                    }
                    break;
                }
                _ => break,
            }
        }
        println!(
            "[compat:nvim] clean_exit={saw_close} exit_code={exit_code:?} bytes={total_bytes}"
        );
    }

    // codex (--version inside PTY)
    {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec!["codex".into(), "--version".into()]),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            })
            .await
            .expect("codex create");
        let mut att = client
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
            .expect("codex attach");
        let (ok, bytes) = wait_for_text(
            &mut att.rx,
            &client,
            att.result.attach_id,
            "codex",
            Duration::from_secs(5),
        )
        .await;
        println!("[compat:codex] ok={ok} bytes={bytes}");
        let _ = client.terminal_kill(&term.address).await;
    }

    // 2. Multi-session concurrency (10 concurrent terminals)
    println!("\n--- 2. Multi-Session Concurrency (10 terminals) ---");
    let mut multi_ids = Vec::new();
    let mut multi_handles = Vec::new();
    let t0 = Instant::now();
    for i in 0..10 {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec!["/bin/sh".into()]),
                cols: Some(80),
                rows: Some(24),
                name: Some(format!("multi-{i}")),
                ..Default::default()
            })
            .await
            .expect("multi create");
        let att = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address.clone(),
                cols: 80,
                rows: 24,
                history: Some(0),
                observe: Some(false),
                resize_notifications: Some(true),
                display: None,
            })
            .await
            .expect("multi attach");
        client
            .terminal_write(
                att.result.attach_id,
                format!("echo MULTI_OK_{i}\n").into_bytes(),
                false,
            )
            .await
            .expect("multi write");
        multi_ids.push(term.address);
        multi_handles.push((i, att));
    }
    let mut all_ok = true;
    for (i, mut att) in multi_handles {
        let (ok, _) = wait_for_text(
            &mut att.rx,
            &client,
            att.result.attach_id,
            &format!("MULTI_OK_{i}"),
            Duration::from_secs(5),
        )
        .await;
        if !ok {
            all_ok = false;
        }
        let _ = client.terminal_detach(att.result.attach_id).await;
    }
    let multi_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let raw_status = client
        .rpc("server.status", serde_json::json!({}))
        .await
        .expect("status");
    let holder_pid = raw_status["holder"]["pid"].as_u64().unwrap_or(0) as u32;
    let core_rss_10 = rss_kb_for_pids(&[core_pid]);
    let holder_rss_10 = rss_kb_for_pids(&[holder_pid]);
    println!(
        "[multi:10] all_ok={all_ok} elapsed_ms={:.2} core_rss_kb={core_rss_10} holder_pid={holder_pid} holder_rss_kb={holder_rss_10}",
        multi_ms
    );

    for id in &multi_ids {
        let _ = client.terminal_kill(id).await;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    let live_after_kill = client.terminal_list(false).await.expect("list").len();
    let core_rss_after = rss_kb_for_pids(&[core_pid]);
    let holder_rss_after = rss_kb_for_pids(&[holder_pid]);
    println!(
        "[multi:cleanup] live_after_kill={live_after_kill} core_rss_kb={core_rss_after} holder_rss_kb={holder_rss_after}"
    );

    // 3. High-throughput burst (5 MiB) + flow control
    println!("\n--- 3. High-Throughput Burst (5 MiB) & Flow Control ---");
    {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "python3 -c 'import sys; line = (\"A\"*79 + \"\\n\")*65536; sys.stdout.write(line); sys.stdout.write(\"BURST_DONE\\n\"); sys.stdout.flush()'"
                        .into(),
                ]),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            })
            .await
            .expect("burst create");
        let mut att = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address.clone(),
                cols: 80,
                rows: 24,
                history: Some(0),
                observe: Some(false),
                resize_notifications: Some(true),
                display: None,
            })
            .await
            .expect("burst attach");
        let t0 = Instant::now();
        let mut total_bytes = 0usize;
        let mut ack_buf = 0u32;
        let mut saw_done = false;
        let mut tail = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            let rem = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(rem, att.rx.recv()).await {
                Ok(Some(OutputStreamFrame::Data(b))) => {
                    total_bytes += b.len();
                    ack_buf += b.len() as u32;
                    if ack_buf >= 32 * 1024 {
                        let _ = client.terminal_ack(att.result.attach_id, ack_buf).await;
                        ack_buf = 0;
                    }
                    let s = String::from_utf8_lossy(&b);
                    tail.push_str(&s);
                    if tail.len() > 256 {
                        tail = tail
                            .chars()
                            .rev()
                            .take(128)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                    if tail.contains("BURST_DONE") {
                        saw_done = true;
                    }
                }
                Ok(Some(OutputStreamFrame::Close(_))) | Ok(None) => break,
                _ => {}
            }
        }
        if ack_buf > 0 {
            let _ = client.terminal_ack(att.result.attach_id, ack_buf).await;
        }
        let elapsed_s = t0.elapsed().as_secs_f64();
        let mb_s = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed_s.max(0.0001);
        let core_rss_burst = rss_kb_for_pids(&[core_pid]);
        let holder_rss_burst = rss_kb_for_pids(&[holder_pid]);
        println!(
            "[burst:5MiB] saw_done={saw_done} received_bytes={total_bytes} elapsed_ms={:.2} throughput_mbs={:.2} core_rss_kb={core_rss_burst} holder_rss_kb={holder_rss_burst}",
            elapsed_s * 1000.0,
            mb_s
        );
    }

    // 3b. Stalled viewer isolation (do NOT send CREDIT, verify channel-0 RPC stays fast)
    {
        let term = client
            .terminal_create(TerminalCreateParams {
                argv: Some(vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "python3 -c 'import sys; chunk=\"X\"*4096+\"\\n\"; [sys.stdout.write(chunk) for _ in range(1500)]'"
                        .into(),
                ]),
                cols: Some(80),
                rows: Some(24),
                ..Default::default()
            })
            .await
            .expect("stall create");
        let _stalled = client
            .terminal_attach(TerminalAttachParams {
                session_id: term.address.clone(),
                cols: 80,
                rows: 24,
                history: Some(0),
                observe: Some(false),
                resize_notifications: Some(true),
                display: None,
            })
            .await
            .expect("stall attach");
        // Deliberately do NOT read or ack _stalled.rx
        tokio::time::sleep(Duration::from_millis(200)).await;
        let rpc_t0 = Instant::now();
        let st_ok = client.server_status().await.is_ok();
        let rpc_ms = rpc_t0.elapsed().as_secs_f64() * 1000.0;
        println!(
            "[flow_control:stalled] ch0_rpc_ok={st_ok} ch0_rpc_ms={:.2}",
            rpc_ms
        );
        let _ = client.terminal_kill(&term.address).await;
    }

    // 4. TypeScript Config Runtime PoC
    println!("\n--- 4. TypeScript Config Runtime (Node.js 24 JSON-RPC stdio) ---");
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sample = manifest_dir
        .parent()
        .unwrap()
        .join("poc/config-runtime/sample.config.ts");
    for run in 1..=3 {
        let res = eval_config_poc(ConfigEvalParams {
            config_path: sample.to_string_lossy().to_string(),
            action_name: Some("greet".into()),
            action_args: Some(serde_json::json!({ "name": "uiterm" })),
            event_name: Some("onAgentStatus".into()),
            event_payload: Some(serde_json::json!({
                "terminal": "t1",
                "status": "blocked",
                "turn": run
            })),
        })
        .await
        .expect("config eval");
        println!(
            "[config:run{run}] startup_ms={:.2} rpc_ms={:.2} rss_mb={:.2}",
            res.startup_ms,
            res.rpc_ms,
            res.worker_rss_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0)
        );
    }

    let _ = status_val;
    daemon.stop().await.expect("stop daemon");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let core_still_alive = rss_kb_for_pids(&[core_pid]) > 0;
    let holder_still_alive = holder_pid > 0 && rss_kb_for_pids(&[holder_pid]) > 0;
    println!(
        "\n[shutdown] core_still_alive={core_still_alive} holder_still_alive={holder_still_alive}"
    );
}
