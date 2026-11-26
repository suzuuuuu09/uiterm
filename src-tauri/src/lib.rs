pub mod commands;
pub mod config_runtime;
pub mod herdr;

use commands::AppState;
use std::sync::Arc;
use tauri::RunEvent;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(AppState::new());
    let exit_state = Arc::clone(&state);

    let app = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
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
            commands::events_unsubscribe,
            commands::config_eval_poc,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            exit_state.shutdown_blocking();
        }
    });
}
