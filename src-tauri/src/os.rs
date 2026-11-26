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
        && std::path::Path::new(&shell).is_file()
    {
        return Some(shell);
    }

    if let Ok(env_shell) = std::env::var("SHELL")
        && !env_shell.is_empty()
        && std::path::Path::new(&env_shell).is_file()
    {
        return Some(env_shell);
    }

    None
}

#[cfg(not(unix))]
pub(crate) fn resolve_os_default_shell() -> Option<String> {
    None
}
