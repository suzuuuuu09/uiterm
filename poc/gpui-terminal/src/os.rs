//! OS-level shell resolution and temporary `herdr-core` `IUTF8` PTY initializer.
//!
//! Mirrors `src-tauri/src/os.rs` and `src-tauri/src/commands.rs` so the GPUI PoC
//! inherits the exact same login-shell detection, XDG restoration, and `IUTF8`
//! canonical-mode Japanese Backspace fix on isolated `herdr-core` v0.1.22 PTYs.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

pub const PTY_IUTF8_INIT_FLAG: &str = "--uiterm-pty-init";

fn is_executable_file(path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = Path::new(path);
        p.is_absolute()
            && p.is_file()
            && std::fs::metadata(p).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[cfg(unix)]
pub fn resolve_os_default_shell() -> Option<String> {
    let mut size = 1024;
    let pw_shell = loop {
        let mut buffer = vec![0; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();

        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                pwd.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if status != 0 || result.is_null() {
            break None;
        }
        let shell = unsafe { (*result).pw_shell };
        if shell.is_null() {
            break None;
        }
        break unsafe { std::ffi::CStr::from_ptr(shell) }
            .to_str()
            .ok()
            .map(str::to_owned);
    };

    if let Some(shell) = pw_shell
        && is_executable_file(&shell)
    {
        return Some(shell);
    }

    if let Ok(env_shell) = std::env::var("SHELL")
        && !env_shell.is_empty()
        && is_executable_file(&env_shell)
    {
        return Some(env_shell);
    }

    None
}

#[cfg(not(unix))]
pub fn resolve_os_default_shell() -> Option<String> {
    None
}

pub fn supports_login_flag(shell: &str) -> bool {
    matches!(
        Path::new(shell).file_name().and_then(|name| name.to_str()),
        Some("fish" | "zsh" | "bash" | "sh" | "dash" | "ksh" | "mksh")
    )
}

pub fn default_shell_argv() -> Vec<String> {
    match resolve_os_default_shell() {
        Some(shell) if supports_login_flag(&shell) => vec![shell, "-l".to_string()],
        Some(shell) => vec![shell],
        None => vec!["/bin/sh".to_string()],
    }
}

/// Enables `IUTF8` on the given terminal file descriptor while preserving all other flags.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn enable_pty_iutf8(fd: libc::c_int) -> std::io::Result<()> {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::zeroed();
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut termios = unsafe { termios.assume_init() };
    if (termios.c_iflag & libc::IUTF8) == 0 {
        termios.c_iflag |= libc::IUTF8;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Enables `IUTF8` on stdin (best-effort) and immediately `exec`s the target command,
/// replacing the process image in-place so the child keeps the same PID/PGID/controlling TTY.
#[cfg(unix)]
pub fn run_pty_iutf8_init(args: &[OsString]) -> ! {
    use std::os::unix::process::CommandExt;

    let mut iter = args.iter();
    let first = iter.next();
    let program = match first {
        Some(arg) if arg == "--" => iter.next(),
        other => other,
    };
    let Some(program) = program else {
        eprintln!("uiterm-gpui-poc: {PTY_IUTF8_INIT_FLAG} requires a target command");
        std::process::exit(2);
    };

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Err(err) = enable_pty_iutf8(libc::STDIN_FILENO) {
        eprintln!("uiterm-gpui-poc: failed to enable IUTF8 on stdin: {err}");
    }

    let err = std::process::Command::new(program).args(iter).exec();
    eprintln!(
        "uiterm-gpui-poc: failed to exec {}: {err}",
        program.to_string_lossy()
    );
    std::process::exit(127);
}

#[cfg(not(unix))]
pub fn run_pty_iutf8_init(_args: &[OsString]) -> ! {
    std::process::exit(2);
}

/// Wraps `argv` with `[exe, "--uiterm-pty-init", "--", ...]` idempotently.
pub fn wrap_argv_with_pty_iutf8_init(mut argv: Vec<String>, exe: &str) -> Vec<String> {
    if argv.is_empty()
        || (argv.len() >= 3 && argv[0] == exe && argv[1] == PTY_IUTF8_INIT_FLAG && argv[2] == "--")
    {
        return argv;
    }
    let mut wrapped = Vec::with_capacity(argv.len() + 3);
    wrapped.push(exe.to_owned());
    wrapped.push(PTY_IUTF8_INIT_FLAG.to_owned());
    wrapped.push("--".to_owned());
    wrapped.append(&mut argv);
    wrapped
}

fn resolve_xdg_dir(env_key: &str, home_subpath: &str) -> Option<String> {
    if let Ok(value) = std::env::var(env_key)
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(home_subpath)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Builds the child environment for shells launched via isolated `herdr-core`,
/// restoring user/QA XDG paths and ensuring UTF-8 locale and color capabilities.
pub fn build_child_shell_env(shell: Option<&str>) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Some(shell_path) = shell {
        env.insert("SHELL".to_string(), shell_path.to_string());
    } else if let Some(resolved) = resolve_os_default_shell() {
        env.insert("SHELL".to_string(), resolved);
    }

    if let Some(xdg_config) = resolve_xdg_dir("XDG_CONFIG_HOME", ".config") {
        env.insert("XDG_CONFIG_HOME".to_string(), xdg_config);
    }
    if let Some(xdg_state) = resolve_xdg_dir("XDG_STATE_HOME", ".local/state") {
        env.insert("XDG_STATE_HOME".to_string(), xdg_state);
    }
    if cfg!(target_os = "linux") {
        for key in ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"] {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.to_string(), value);
            }
        }
    }

    let missing_lang = std::env::var("LANG")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true);
    if missing_lang {
        let fallback_lang = std::env::var("LC_CTYPE")
            .ok()
            .filter(|v| v.to_uppercase().contains("UTF-8") || v.to_uppercase().contains("UTF8"))
            .unwrap_or_else(|| {
                if cfg!(target_os = "linux") {
                    "C.UTF-8"
                } else {
                    "en_US.UTF-8"
                }
                .to_owned()
            });
        env.insert("LANG".to_string(), fallback_lang);
    }

    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert("TERM_PROGRAM".to_string(), "uiterm-gpui-poc".to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_argv_is_idempotent_and_preserves_unrelated_flags() {
        let exe = "/tmp/uiterm-gpui-poc";
        let argv = vec!["/bin/fish".to_string(), "-l".to_string()];
        let wrapped = wrap_argv_with_pty_iutf8_init(argv, exe);
        assert_eq!(
            wrapped,
            vec![exe, PTY_IUTF8_INIT_FLAG, "--", "/bin/fish", "-l"]
        );
        let twice = wrap_argv_with_pty_iutf8_init(wrapped.clone(), exe);
        assert_eq!(twice, wrapped);

        let unrelated = vec![
            "/usr/bin/env".to_string(),
            PTY_IUTF8_INIT_FLAG.to_string(),
            "--".to_string(),
            "echo".to_string(),
        ];
        let wrapped_unrelated = wrap_argv_with_pty_iutf8_init(unrelated, exe);
        assert_eq!(wrapped_unrelated[0], exe);
        assert_eq!(wrapped_unrelated.len(), 7);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn enable_pty_iutf8_sets_flag_and_preserves_existing_iflags() {
        use std::os::fd::FromRawFd;

        let (mut master, mut slave) = (0, 0);
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
        let _master = unsafe { std::fs::File::from_raw_fd(master) };
        let _slave = unsafe { std::fs::File::from_raw_fd(slave) };

        let mut before = std::mem::MaybeUninit::<libc::termios>::zeroed();
        assert_eq!(unsafe { libc::tcgetattr(slave, before.as_mut_ptr()) }, 0);
        let before = unsafe { before.assume_init() };

        enable_pty_iutf8(slave).expect("enable_pty_iutf8");

        let mut after = std::mem::MaybeUninit::<libc::termios>::zeroed();
        assert_eq!(unsafe { libc::tcgetattr(slave, after.as_mut_ptr()) }, 0);
        let after = unsafe { after.assume_init() };

        assert_ne!(after.c_iflag & libc::IUTF8, 0);
        assert_eq!(after.c_iflag & !libc::IUTF8, before.c_iflag & !libc::IUTF8);
    }

    #[test]
    fn child_env_restores_xdg_and_terminal_vars() {
        let env = build_child_shell_env(Some("/bin/zsh"));
        assert_eq!(env.get("SHELL").map(String::as_str), Some("/bin/zsh"));
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
        let expected = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                let home = std::env::var("HOME")
                    .ok()
                    .filter(|value| !value.trim().is_empty())?;
                Some(
                    Path::new(&home)
                        .join(".config")
                        .to_string_lossy()
                        .into_owned(),
                )
            });
        assert_eq!(env.get("XDG_CONFIG_HOME"), expected.as_ref());
    }
}
