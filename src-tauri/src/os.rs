#[cfg(unix)]
fn is_executable_file(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let path = std::path::Path::new(path);
    path.is_absolute()
        && path.is_file()
        && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(unix)]
pub(crate) fn resolve_os_default_shell() -> Option<String> {
    let mut size = 1024;
    let pw_shell = loop {
        let mut buffer = vec![0; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();

        // The reentrant lookup stores the record and strings in our local buffers.
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
pub(crate) fn resolve_os_default_shell() -> Option<String> {
    None
}

/// Internal argv flag for the temporary `IUTF8` PTY initializer workaround.
///
/// `herdr-core` v0.1.22 opens new Unix PTYs without setting `termios.c_iflag |= IUTF8`,
/// which causes canonical-mode Backspace to erase 1 byte instead of a UTF-8 character.
/// Remove this flag and helper once upstream `herdr-core` sets `IUTF8` natively.
pub const PTY_IUTF8_INIT_FLAG: &str = "--uiterm-pty-init";

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
/// replacing the process image so the child inherits the same PID/PGID/controlling TTY.
#[cfg(unix)]
pub fn run_pty_iutf8_init(args: &[std::ffi::OsString]) -> ! {
    use std::os::unix::process::CommandExt;

    let mut iter = args.iter();
    let first = iter.next();
    let program = match first {
        Some(arg) if arg == "--" => iter.next(),
        other => other,
    };
    let Some(program) = program else {
        eprintln!("uiterm: {PTY_IUTF8_INIT_FLAG} requires a target command");
        std::process::exit(2);
    };

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Err(err) = enable_pty_iutf8(libc::STDIN_FILENO) {
        eprintln!("uiterm: failed to enable IUTF8 on stdin: {err}");
    }

    let err = std::process::Command::new(program).args(iter).exec();
    eprintln!(
        "uiterm: failed to exec {}: {err}",
        program.to_string_lossy()
    );
    std::process::exit(127);
}

/// Returns true if `program` refers to the current executable or a `uiterm` binary.
fn is_uiterm_wrapper_exe(program: &str, current_exe: Option<&str>) -> bool {
    if current_exe.is_some_and(|exe| program == exe) {
        return true;
    }
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "uiterm" || name == "uiterm.exe")
}

/// Strips the internal `[<exe>, "--uiterm-pty-init", "--", ...]` prefix from reported argv
/// only when `argv[0]` identifies the uiterm executable.
pub(crate) fn strip_pty_iutf8_wrapper(argv: Vec<String>) -> Vec<String> {
    let current_exe = std::env::current_exe().ok();
    strip_pty_iutf8_wrapper_for_exe(argv, current_exe.as_deref().and_then(|p| p.to_str()))
}

fn strip_pty_iutf8_wrapper_for_exe(argv: Vec<String>, current_exe: Option<&str>) -> Vec<String> {
    if argv.len() >= 4
        && is_uiterm_wrapper_exe(&argv[0], current_exe)
        && argv[1] == PTY_IUTF8_INIT_FLAG
        && argv[2] == "--"
    {
        argv[3..].to_vec()
    } else {
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_pty_iutf8_wrapper_recovers_original_argv() {
        let exe = "/Applications/uiterm.app/Contents/MacOS/uiterm";
        let wrapped = vec![
            exe.to_string(),
            PTY_IUTF8_INIT_FLAG.to_string(),
            "--".to_string(),
            "/bin/fish".to_string(),
            "-l".to_string(),
        ];
        assert_eq!(
            strip_pty_iutf8_wrapper_for_exe(wrapped, Some(exe)),
            vec!["/bin/fish".to_string(), "-l".to_string()]
        );

        let unrelated = vec![
            "/bin/echo".to_string(),
            PTY_IUTF8_INIT_FLAG.to_string(),
            "--".to_string(),
            "hello".to_string(),
        ];
        assert_eq!(
            strip_pty_iutf8_wrapper_for_exe(unrelated.clone(), Some(exe)),
            unrelated
        );

        let plain = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo hi".to_string(),
        ];
        assert_eq!(
            strip_pty_iutf8_wrapper_for_exe(plain.clone(), Some(exe)),
            plain
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn enable_pty_iutf8_sets_flag_and_preserves_other_iflags() {
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

        assert_ne!(after.c_iflag & libc::IUTF8, 0, "IUTF8 must be enabled");
        assert_eq!(
            after.c_iflag & !libc::IUTF8,
            before.c_iflag & !libc::IUTF8,
            "existing c_iflag bits must be preserved"
        );
    }
}
