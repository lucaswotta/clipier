//! The Leme Pier window. Every command is a thin wrapper over `leme-core`; if one grows logic, the
//! logic belongs in the core.
// Tauri hands a command its arguments by value, after deserializing them from the interface.
#![allow(clippy::needless_pass_by_value)]

mod commands;
mod selftest;

fn main() {
    tauri::Builder::default()
        .on_page_load(|webview, payload| {
            if selftest::requested().is_some() && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                selftest::start(webview);
            }
        })
        .manage(commands::Agents::default())
        .manage(commands::Chats::default())
        .manage(commands::Races::default())
        .manage(commands::Crews::default())
        .invoke_handler(tauri::generate_handler![
            commands::detect_agents,
            commands::usage_summary,
            commands::chat::chat_list,
            commands::chat::chat_create,
            commands::chat::chat_view,
            commands::chat::chat_send,
            commands::chat::chat_cancel,
            commands::race::race_check,
            commands::race::race_improve,
            commands::race::race_start,
            commands::race::race_list,
            commands::race::race_view,
            commands::race::race_diff,
            commands::race::race_cancel,
            commands::crew::crew_plan,
            commands::crew::crew_stop_planning,
            commands::crew::crew_start,
            commands::crew::crew_list,
            commands::crew::crew_view,
            commands::crew::crew_command,
            commands::crew::crew_diff,
            selftest::selftest_report,
            commands::roles::roles_list,
            commands::roles::role_save,
            commands::roles::project_settings,
            commands::roles::notes_list,
            commands::roles::note_forget,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Leme Pier");
}

/// Drives the real command handler and the real capability files through Tauri's mock runtime, so
/// that a permission or security change that stops the interface from calling Leme is caught here.
#[cfg(test)]
mod tests {
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;
    use tauri::{WebviewWindowBuilder, generate_context, generate_handler};

    fn call(command: &str, body: serde_json::Value) -> Result<InvokeResponseBody, serde_json::Value> {
        let app = mock_builder()
            .invoke_handler(generate_handler![crate::commands::usage_summary])
            .build(generate_context!())
            .expect("the app builds with the real configuration");
        let window = WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default()).build().unwrap();
        get_ipc_response(
            &window,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                // The interface's own origin, which Windows writes differently.
                url: if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" }.parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: tauri::http::HeaderMap::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
    }

    #[test]
    fn the_interface_can_call_a_command_with_the_configured_capabilities() {
        let answer = call("usage_summary", serde_json::json!({})).expect("the command is allowed");
        let InvokeResponseBody::Json(text) = answer else { panic!("a command answers with JSON") };
        assert!(serde_json::from_str::<serde_json::Value>(&text).unwrap().is_array());
    }

    #[test]
    fn the_core_apis_the_interface_does_not_need_are_denied() {
        let error =
            call("plugin:event|listen", serde_json::json!({ "event": "x", "target": { "kind": "Any" }, "handler": 0 }))
                .unwrap_err();
        assert!(error.to_string().contains("not allowed"), "got: {error}");
    }
}
