//! The Leme Pier window. Every command is a thin wrapper over `leme-core`; if one grows logic, the
//! logic belongs in the core.

mod commands;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![commands::detect_agents, commands::usage_summary])
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
