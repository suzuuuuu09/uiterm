mod app_state;
#[cfg(test)]
mod bindings;
pub mod commands;
pub mod config_runtime;
pub mod herdr;
pub mod os;

use app_state::AppState;
use std::sync::Arc;
use tauri::{RunEvent, WebviewWindowBuilder, webview::NewWindowResponse};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(AppState::default());
    let exit_state = Arc::clone(&state);

    let app = register_ipc(tauri::Builder::default())
        .manage(state)
        .setup(|app| {
            use tokio::signal::unix::{SignalKind, signal};

            let (mut terminate, mut interrupt) = tauri::async_runtime::block_on(async {
                Ok::<_, std::io::Error>((
                    signal(SignalKind::terminate())?,
                    signal(SignalKind::interrupt())?,
                ))
            })?;
            let dev_url = app.config().build.dev_url.clone().filter(|_| cfg!(dev));
            let window = &app.config().app.windows[0];
            WebviewWindowBuilder::from_config(app, window)?
                .on_navigation(move |url| allow_navigation(url, dev_url.as_ref()))
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .build()?;

            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::select! {
                    _ = terminate.recv() => app.exit(143),
                    _ = interrupt.recv() => app.exit(130),
                }
            });
            Ok(())
        })
        .build(app_context())
        .expect("error while building tauri application");

    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            exit_state.shutdown_blocking();
        }
    });
}

fn allow_navigation(url: &tauri::Url, dev_url: Option<&tauri::Url>) -> bool {
    (url.scheme() == "tauri" && url.host_str() == Some("localhost") && url.port().is_none())
        || dev_url.is_some_and(|dev_url| url.origin() == dev_url.origin())
}

fn app_context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

fn register_ipc<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::core_status,
        commands::core_reconnect,
        commands::terminal_create,
        commands::terminal_list,
        commands::terminal_attach,
        commands::terminal_detach,
        commands::terminal_write,
        commands::terminal_resize,
        commands::terminal_focus,
        commands::terminal_facts,
        commands::terminal_ack,
        commands::terminal_kill,
        commands::events_subscribe,
        commands::events_ack,
        commands::events_unsubscribe,
        commands::config_eval_poc,
    ])
}

#[cfg(test)]
mod ipc_tests {
    use super::*;
    use tauri::Manager;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;

    #[test]
    fn navigation_uses_only_the_embedded_or_exact_development_origin() {
        let dev = "http://localhost:1420".parse().unwrap();
        for (url, release, development) in [
            ("tauri://localhost/index.html", true, true),
            ("http://localhost:1420/", false, true),
            ("http://localhost:1421/", false, false),
            ("https://localhost:1420/", false, false),
            ("tauri://other/", false, false),
        ] {
            let url = url.parse().unwrap();
            assert_eq!(allow_navigation(&url, None), release);
            assert_eq!(allow_navigation(&url, Some(&dev)), development);
        }
    }

    #[test]
    fn only_local_main_can_invoke_native_commands() {
        let app = register_ipc(mock_builder())
            .manage(Arc::new(AppState::default()))
            .build(app_context())
            .unwrap();
        let main = app.get_webview_window("main").unwrap_or_else(|| {
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap()
        });
        let other = tauri::WebviewWindowBuilder::new(&app, "other", Default::default())
            .build()
            .unwrap();

        let write = |url: &str| InvokeRequest {
            cmd: "terminal_write".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().unwrap(),
            body: InvokeBody::Raw(vec![0, 255]),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        let invalid = get_ipc_response(&main, write("tauri://localhost")).unwrap_err();
        assert_eq!(invalid["code"], "invalid_args");
        for (window, origin) in [
            (&main, "https://example.invalid"),
            (&other, "tauri://localhost"),
        ] {
            let denied = get_ipc_response(window, write(origin)).unwrap_err();
            assert!(denied.is_string(), "{denied}");
        }
    }
}
