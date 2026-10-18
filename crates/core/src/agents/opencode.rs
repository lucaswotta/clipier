//! `OpenCode`, `opencode run --format json`.

use serde_json::Value;

use super::{Assembler, Event, field, number, summarize, text};

/// The error object carries its message under `message` or `data.message`; anything else is shown raw.
fn error_message(error: &Value) -> String {
    ["/message", "/data/message"]
        .iter()
        .find_map(|pointer| error.pointer(pointer).and_then(Value::as_str))
        .or_else(|| error.as_str())
        .map_or_else(|| error.to_string(), str::to_owned)
}

pub(super) fn handle(event: &Value, out: &mut Assembler) -> bool {
    let part = &event["part"];
    out.session(field(event, "sessionID"));
    match field(event, "type") {
        "step_start" => {}
        "step_finish" => out.tokens(
            number(part, "/tokens/input"),
            number(part, "/tokens/cache/read"),
            number(part, "/tokens/output"),
        ),
        "text" => out.message(text(part, "/text")),
        "tool_use" => {
            let name = text(part, "/tool");
            let input = &part["state"]["input"];
            match name.as_str() {
                "bash" | "shell" => out.command(text(input, "/command"), text(part, "/state/output")),
                "edit" | "write" | "patch" => out.edit(text(input, "/filePath")),
                _ => out.push(Event::Tool { summary: summarize(input), name }),
            }
        }
        "error" => out.error(error_message(&event["error"])),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::agents::{Agent, Event, command, message, transcribe};

    #[test]
    fn a_command_arrives_with_its_output_in_one_event() {
        let t = transcribe(
            Agent::Opencode,
            &[
                r#"{"type":"step_start","part":{}}"#,
                r#"{"type":"tool_use","part":{"tool":"shell","state":{"input":{"command":"ls -la"},"output":"total 0"}}}"#,
                r#"{"type":"step_finish","part":{"tokens":{"input":30,"output":2}}}"#,
                r#"{"type":"text","part":{"text":"Two files."}}"#,
            ],
        );
        assert_eq!(t.events(), [command("ls -la", "total 0"), message("Two files.")]);
        assert_eq!((t.tokens_in(), t.tokens_cached(), t.tokens_out(), t.unrecognized()), (30, 0, 2, 0));
    }

    #[test]
    fn edits_other_tools_and_errors_become_their_own_events() {
        let t = transcribe(
            Agent::Opencode,
            &[
                r#"{"type":"tool_use","part":{"tool":"edit","state":{"input":{"filePath":"/w/a.rs"}}}}"#,
                r#"{"type":"tool_use","part":{"tool":"grep","state":{"input":{"pattern":"todo"}}}}"#,
                r#"{"type":"error","error":{"name":"APIError","data":{"message":"rate limited"}}}"#,
                r#"{"type":"error","error":"plain text failure"}"#,
            ],
        );
        assert_eq!(
            t.events(),
            [
                Event::Edit { path: "/w/a.rs".into() },
                Event::Tool { name: "grep".into(), summary: "todo".into() },
                Event::Error { text: "rate limited".into() },
                Event::Error { text: "plain text failure".into() },
            ]
        );
    }

    #[test]
    fn an_unknown_event_type_is_counted_as_unrecognized() {
        let t = transcribe(Agent::Opencode, &[r#"{"type":"something_new"}"#]);
        assert_eq!((t.events().len(), t.unrecognized()), (0, 1));
    }

    #[test]
    fn reports_the_session_id_and_the_last_text_as_the_answer() {
        let t = transcribe(
            Agent::Opencode,
            &[
                r#"{"type":"step_start","sessionID":"ses_1","part":{}}"#,
                r#"{"type":"text","sessionID":"ses_1","part":{"text":"first"}}"#,
                r#"{"type":"text","sessionID":"ses_1","part":{"text":"last"}}"#,
            ],
        );
        assert_eq!((t.session_id(), t.final_text()), (Some("ses_1"), Some("last")));
    }

    #[test]
    fn two_commands_in_a_row_keep_their_own_output() {
        let t = transcribe(
            Agent::Opencode,
            &[
                r#"{"type":"tool_use","part":{"tool":"bash","state":{"input":{"command":"a"},"output":"1"}}}"#,
                r#"{"type":"tool_use","part":{"tool":"bash","state":{"input":{"command":"b"},"output":"2"}}}"#,
            ],
        );
        assert_eq!(t.events(), [command("a", "1"), command("b", "2")]);
    }
}
