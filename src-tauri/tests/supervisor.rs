use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use uiterm_lib::herdr::{HerdrClient, TerminalCreateParams};

fn supervisor() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uiterm"));
    command
        .arg("--uiterm-core-supervisor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    command
}

async fn ready(child: &mut Child) -> Value {
    let mut reply = String::new();
    timeout(
        Duration::from_secs(12),
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut reply),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_str(&reply).unwrap()
}

#[tokio::test]
async fn rejects_wrong_core_before_serving() {
    let mut child = supervisor()
        .env("HERDR_CORE_BIN", "/bin/sh")
        .spawn()
        .unwrap();
    let reply = ready(&mut child).await;
    assert_eq!(reply["Err"]["code"], "core_version_mismatch");
    assert!(!child.wait().await.unwrap().success());
}

#[tokio::test]
async fn gui_pipe_eof_cleans_up_after_core_crash_and_leaves_other_instance_running() {
    let mut gui = supervisor().spawn().unwrap();
    let reply = ready(&mut gui).await;
    let (socket, session): (PathBuf, String) = serde_json::from_value(reply["Ok"].clone()).unwrap();

    let mut other = supervisor().spawn().unwrap();
    let reply = ready(&mut other).await;
    let (other_socket, other_session): (PathBuf, String) =
        serde_json::from_value(reply["Ok"].clone()).unwrap();
    let other_client = HerdrClient::connect(&other_socket, other_session)
        .await
        .unwrap();
    let other_pid = other_client.server_status().await.unwrap().pid;

    let client = HerdrClient::connect(&socket, &session).await.unwrap();
    let core_pid = client.server_status().await.unwrap().pid;
    let terminal = client
        .terminal_create(TerminalCreateParams {
            argv: Some(vec!["/bin/sh".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
    let shell_pid = terminal.pid.unwrap();
    let status = client
        .rpc("server.status", serde_json::json!({}))
        .await
        .unwrap();
    let holder_pid = i32::try_from(status["holder"]["pid"].as_i64().unwrap()).unwrap();

    // This PID is the current core reached through the new test instance's private socket.
    assert!(core_pid > 0, "expected positive core PID, got {core_pid}");
    let core_pid_i32 = i32::try_from(core_pid).unwrap();
    assert_eq!(unsafe { libc::kill(core_pid_i32, libc::SIGKILL) }, 0);
    let recovered = timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(client) = HerdrClient::connect(&socket, &session).await
                && let Ok(status) = client.server_status().await
                && status.pid != core_pid
            {
                return (client, status.pid);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let (recovered, second_core_pid) = recovered;
    assert!(
        recovered
            .terminal_list(false)
            .await
            .unwrap()
            .iter()
            .any(|t| t.pid == Some(shell_pid))
    );

    // Crash the recovered core a second time to ensure the supervision loop keeps re-arming.
    assert!(
        second_core_pid > 0,
        "expected positive recovered core PID, got {second_core_pid}"
    );
    let second_core_pid_i32 = i32::try_from(second_core_pid).unwrap();
    assert_eq!(unsafe { libc::kill(second_core_pid_i32, libc::SIGKILL) }, 0);
    let (recovered_again, third_core_pid) = timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(client) = HerdrClient::connect(&socket, &session).await
                && let Ok(status) = client.server_status().await
                && status.pid != second_core_pid
            {
                return (client, status.pid);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        third_core_pid > 0,
        "expected positive third core PID, got {third_core_pid}"
    );
    let third_core_pid_i32 = i32::try_from(third_core_pid).unwrap();
    assert!(
        recovered_again
            .terminal_list(false)
            .await
            .unwrap()
            .iter()
            .any(|t| t.pid == Some(shell_pid))
    );

    drop(gui.stdin.take());
    assert!(
        timeout(Duration::from_secs(16), gui.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!socket.parent().unwrap().exists());
    timeout(Duration::from_secs(3), async {
        for pid in [
            core_pid_i32,
            second_core_pid_i32,
            third_core_pid_i32,
            holder_pid,
            shell_pid,
        ] {
            while unsafe { libc::kill(pid, 0) } == 0 {
                sleep(Duration::from_millis(25)).await;
            }
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    })
    .await
    .unwrap();
    assert_eq!(other_client.server_status().await.unwrap().pid, other_pid);

    drop(other.stdin.take());
    assert!(
        timeout(Duration::from_secs(16), other.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!other_socket.parent().unwrap().exists());
}

#[tokio::test]
async fn core_exiting_during_start_releases_private_state() {
    use std::os::unix::fs::PermissionsExt;

    let mut template = *b"/tmp/uiterm-start-test.XXXXXXXX\0";
    let path = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
    assert!(!path.is_null());
    let directory = PathBuf::from(std::str::from_utf8(&template[..template.len() - 1]).unwrap());
    let executable = directory.join("fake-core");
    let report = directory.join("socket-path");
    std::fs::write(
        &executable,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then\n  echo hc 0.1.22\n  exit 0\nfi\nprintf '%s' \"$HC_SOCKET\" > \"$UITERM_TEST_START_REPORT\"\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut child = supervisor()
        .env("HERDR_CORE_BIN", &executable)
        .env("UITERM_TEST_START_REPORT", &report)
        .spawn()
        .unwrap();
    let reply = ready(&mut child).await;
    let socket = PathBuf::from(std::fs::read_to_string(&report).unwrap());
    std::fs::remove_file(report).unwrap();
    std::fs::remove_file(executable).unwrap();
    std::fs::remove_dir(directory).unwrap();

    assert_eq!(reply["Err"]["code"], "core_start_failed");
    assert!(!child.wait().await.unwrap().success());
    assert!(!socket.parent().unwrap().exists());
}
