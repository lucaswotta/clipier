//! Codex, `codex exec --json`.

use serde_json::Value;

use super::{Assembler, field, number, text};

/// Codex wraps every command in `/bin/bash -lc '...'`; only the inside matters.
fn without_shell(command: &str) -> String {
    command
        .strip_prefix("/bin/bash -lc ")
        .map_or_else(|| command.to_owned(), |rest| rest.trim_matches('\'').replace("'\\''", "'"))
}

pub(super) fn handle(event: &Value, out: &mut Assembler) -> bool {
    let item = &event["item"];
    let key = text(item, "/id");
    out.session(field(event, "thread_id"));
    match (field(event, "type"), field(item, "type")) {
        ("turn.completed", _) => {
            // `input_tokens` already includes what was served from the cache.
            let input = number(event, "/usage/input_tokens");
            let cached = number(event, "/usage/cached_input_tokens").min(input);
            out.tokens(input - cached, cached, number(event, "/usage/output_tokens"));
        }
        ("turn.failed", _) => out.error(text(event, "/error/message")),
        ("error", _) => out.error(text(event, "/message")),
        ("item.completed", "agent_message") => out.message(text(item, "/text")),
        ("item.started", "command_execution") => out.command_started(key, without_shell(&text(item, "/command"))),
        ("item.completed", "command_execution") => {
            let output = text(item, "/aggregated_output");
            if !out.command_finished(&key, output.clone()) {
                out.command(without_shell(&text(item, "/command")), output);
            }
        }
        ("item.completed", "file_change") => {
            for change in item["changes"].as_array().into_iter().flatten() {
                out.edit(text(change, "/path"));
            }
        }
        ("thread.started" | "turn.started" | "item.started" | "item.updated" | "item.completed", _) => {}
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::agents::{Agent, Event, command, message, transcribe};

    #[test]
    fn links_the_start_and_the_end_of_a_command() {
        let t = transcribe(
            Agent::Codex,
            &[
                r#"{"type":"thread.started","thread_id":"x"}"#,
                r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Listing."}}"#,
                r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'ls -1A'","aggregated_output":"","exit_code":null}}"#,
                r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'ls -1A'","aggregated_output":"a\nb\n","exit_code":0}}"#,
                r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"/w/x.py","kind":"update"}]}}"#,
                r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":7}}"#,
            ],
        );
        assert_eq!(
            t.events(),
            [message("Listing."), command("ls -1A", "a\nb\n"), Event::Edit { path: "/w/x.py".into() }]
        );
        assert_eq!((t.tokens_in(), t.tokens_cached(), t.tokens_out(), t.unrecognized()), (20, 80, 7, 0));
    }

    #[test]
    fn reports_the_thread_id_as_the_session_and_the_last_message_as_the_answer() {
        let t = transcribe(
            Agent::Codex,
            &[
                r#"{"type":"thread.started","thread_id":"th-9"}"#,
                r#"{"type":"item.completed","item":{"id":"i0","type":"agent_message","text":"first"}}"#,
                r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"last"}}"#,
            ],
        );
        assert_eq!((t.session_id(), t.final_text()), (Some("th-9"), Some("last")));
    }

    #[test]
    fn a_command_that_only_shows_up_completed_is_still_recorded() {
        let t = transcribe(
            Agent::Codex,
            &[
                r#"{"type":"item.completed","item":{"id":"c","type":"command_execution","command":"/bin/bash -lc 'echo it'\\''s'","aggregated_output":"it's\n","exit_code":0}}"#,
            ],
        );
        assert_eq!(t.events(), [command("echo it's", "it's\n")]);
    }

    #[test]
    fn failures_become_error_events() {
        let t = transcribe(
            Agent::Codex,
            &[r#"{"type":"turn.failed","error":{"message":"quota"}}"#, r#"{"type":"error","message":"boom"}"#],
        );
        assert_eq!(t.events(), [Event::Error { text: "quota".into() }, Event::Error { text: "boom".into() }]);
    }
}
