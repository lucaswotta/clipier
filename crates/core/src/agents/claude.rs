//! Claude Code, `--output-format stream-json`.

use serde_json::Value;

use super::{Assembler, Event, field, number, summarize, text};

fn parts(event: &Value) -> impl Iterator<Item = &Value> {
    event["message"]["content"].as_array().into_iter().flatten()
}

/// Tool results come as plain text or as a list of text blocks.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

pub(super) fn handle(event: &Value, out: &mut Assembler) -> bool {
    out.session(field(event, "session_id"));
    match field(event, "type") {
        "system" | "rate_limit_event" => {}
        "assistant" => {
            let mut known = true;
            for part in parts(event) {
                match field(part, "type") {
                    "text" => out.message(text(part, "/text")),
                    "tool_use" => tool_use(part, out),
                    "thinking" | "redacted_thinking" => {}
                    _ => known = false,
                }
            }
            return known;
        }
        "user" => {
            for part in parts(event) {
                if field(part, "type") == "tool_result" {
                    out.command_finished(&text(part, "/tool_use_id"), result_text(&part["content"]));
                }
            }
        }
        "result" => {
            // Writing the cache is billed above the normal rate and reading it far below, so only
            // the reads count as cached.
            let fresh = number(event, "/usage/input_tokens")
                .saturating_add(number(event, "/usage/cache_creation_input_tokens"));
            out.tokens(fresh, number(event, "/usage/cache_read_input_tokens"), number(event, "/usage/output_tokens"));
            if event["is_error"].as_bool() == Some(true) {
                out.error(text(event, "/result"));
            } else if let Some(answer) = event["result"].as_str() {
                out.final_answer(answer.to_owned());
            }
        }
        _ => return false,
    }
    true
}

fn tool_use(part: &Value, out: &mut Assembler) {
    let key = text(part, "/id");
    let name = text(part, "/name");
    let input = &part["input"];
    match name.as_str() {
        "Bash" => out.command_started(key, text(input, "/command")),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => out.edit(text(input, "/file_path")),
        _ => out.push(Event::Tool { summary: summarize(input), name }),
    }
}

#[cfg(test)]
mod tests {
    use crate::agents::{Agent, command, message, transcribe};

    #[test]
    fn fills_a_command_output_by_the_tool_id() {
        let t = transcribe(
            Agent::Claude,
            &[
                r#"{"type":"system","subtype":"init"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git ls-files bin/"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"README.md"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#,
                r#"{"type":"result","is_error":false,"usage":{"input_tokens":4,"cache_read_input_tokens":10,"output_tokens":3}}"#,
            ],
        );
        assert_eq!(t.events(), [command("git ls-files bin/", "README.md"), message("Done.")]);
        assert_eq!((t.tokens_in(), t.tokens_cached(), t.tokens_out(), t.unrecognized()), (4, 10, 3, 0));
    }

    #[test]
    fn writing_the_cache_is_fresh_input_and_reading_it_is_cached() {
        let t = transcribe(
            Agent::Claude,
            &[
                r#"{"type":"result","is_error":false,"usage":{"input_tokens":2,"cache_creation_input_tokens":500,"cache_read_input_tokens":900,"output_tokens":1}}"#,
            ],
        );
        assert_eq!((t.tokens_in(), t.tokens_cached()), (502, 900));
    }

    #[test]
    fn reads_list_style_tool_results_and_reports_a_failed_run() {
        let t = transcribe(
            Agent::Claude,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}]}}"#,
                r#"{"type":"result","is_error":true,"result":"rate limit reached","usage":{}}"#,
            ],
        );
        assert_eq!(t.events()[0], command("ls", "a\nb"));
        assert_eq!(t.events()[1], crate::agents::Event::Error { text: "rate limit reached".into() });
    }

    #[test]
    fn an_unknown_content_block_is_unrecognized_but_the_known_ones_beside_it_are_kept() {
        let t = transcribe(
            Agent::Claude,
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"hologram"}]}}"#],
        );
        assert_eq!(t.events(), [message("hi")]);
        assert_eq!(t.unrecognized(), 1);
    }

    #[test]
    fn reports_the_session_id_and_the_final_answer() {
        let t = transcribe(
            Agent::Claude,
            &[
                r#"{"type":"system","subtype":"init","session_id":"s-1"}"#,
                r#"{"type":"assistant","session_id":"s-1","message":{"content":[{"type":"text","text":"working"}]}}"#,
                r#"{"type":"result","session_id":"s-1","is_error":false,"result":"the answer","usage":{}}"#,
            ],
        );
        assert_eq!((t.session_id(), t.final_text()), (Some("s-1"), Some("the answer")));
    }

    #[test]
    fn edits_and_other_tools_become_their_own_events() {
        let t = transcribe(
            Agent::Claude,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Write","input":{"file_path":"/w/x.rs"}},{"type":"tool_use","id":"b","name":"Grep","input":{"pattern":"todo"}}]}}"#,
            ],
        );
        assert_eq!(
            t.events(),
            [
                crate::agents::Event::Edit { path: "/w/x.rs".into() },
                crate::agents::Event::Tool { name: "Grep".into(), summary: "todo".into() }
            ]
        );
    }
}
