#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(unix)]
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(uiterm_lib::os::PTY_IUTF8_INIT_FLAG))
    {
        let rest: Vec<std::ffi::OsString> = std::env::args_os().skip(2).collect();
        uiterm_lib::os::run_pty_iutf8_init(&rest);
    }
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(
            uiterm_lib::herdr::process::SUPERVISOR_FLAG,
        ))
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("cannot start supervisor runtime");
        if let Err(err) = runtime.block_on(uiterm_lib::herdr::process::run_supervisor()) {
            eprintln!("uiterm: {}", err.code);
            std::process::exit(1);
        }
        return;
    }
    uiterm_lib::run();
}
