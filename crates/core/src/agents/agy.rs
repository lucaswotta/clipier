//! Antigravity, `agy -p --output-format stream-json`.

use serde_json::Value;

use super::{Assembler, Event, field, number, summarize, text};

pub(super) fn handle(event: &Value, out: &mut Assembler) -> bool {
    match field(event, "event") {
        "init" => out.session(&text(event, "/conversation_id")),
        "result" => {
            out.session(&text(event, "/result/conversation_id"));
            if let Some(answer) = event.pointer("/result/response").and_then(Value::as_str) {
                out.final_answer(answer.to_owned());
            }
            out.tokens(number(event, "/result/usage/input_tokens"), 0, number(event, "/result/usage/output_tokens"));
            let status = text(event, "/result/status");
            if status != "SUCCESS" {
                out.error(format!("agy finished with {status}"));
            }
        }
        "step_update" => return step(&event["step_update"], out),
        _ => return false,
    }
    true
}

fn step(step: &Value, out: &mut Assembler) -> bool {
    let key = format!("step-{}", number(step, "/step_index"));
    match field(step, "step_type") {
        "user_input" => {}
        "agent_response" => {
            let fragment = text(step, "/text_delta");
            if !fragment.is_empty() {
                out.reply_fragment(key, fragment);
            }
        }
        "tool" => {
            let info = &step["tool_info"];
            if !out.has(&key) {
                tool_started(&key, info, out);
            }
            if let Some(output) = info["output"].as_str() {
                out.command_finished(&key, output.replace("\r\n", "\n"));
            }
        }
        _ => return false,
    }
    true
}

fn tool_started(key: &str, info: &Value, out: &mut Assembler) {
    let name = text(info, "/name");
    let parameters = &info["parameters"];
    match name.as_str() {
        "run_command" => out.command_started(key.to_owned(), text(parameters, "/CommandLine")),
        "write_to_file" | "replace_file_content" | "multi_replace_file_content" => {
            out.edit(text(parameters, "/TargetFile"));
        }
        _ => out.push(Event::Tool { summary: summarize(parameters), name }),
    }
}

#[cfg(test)]
mod tests {
    use crate::agents::{Agent, Event, command, message, transcribe};

    #[test]
    fn joins_the_reply_pieces_and_the_command_output() {
        let t = transcribe(
            Agent::Agy,
            &[
                r#"{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"List"}}"#,
                r#"{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"ing."}}"#,
                r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"ls bin/"}}}}"#,
                r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"ls bin/"},"output":"build\r\n"}}}"#,
                r#"{"event":"result","result":{"status":"SUCCESS","usage":{"input_tokens":50,"output_tokens":5}}}"#,
            ],
        );
        assert_eq!(t.events(), [message("Listing."), command("ls bin/", "build\n")]);
        assert_eq!((t.tokens_in(), t.tokens_cached(), t.tokens_out(), t.unrecognized()), (50, 0, 5, 0));
    }

    #[test]
    fn reports_the_conversation_id_and_the_final_response() {
        let t = transcribe(
            Agent::Agy,
            &[
                r#"{"event":"init","conversation_id":"c-5","init":{}}"#,
                r#"{"event":"step_update","step_update":{"step_index":1,"step_type":"agent_response","text_delta":"part"}}"#,
                r#"{"event":"result","result":{"conversation_id":"c-5","status":"SUCCESS","response":"whole answer","usage":{}}}"#,
            ],
        );
        assert_eq!((t.session_id(), t.final_text()), (Some("c-5"), Some("whole answer")));
    }

    #[test]
    fn a_run_that_did_not_succeed_is_reported() {
        let t = transcribe(Agent::Agy, &[r#"{"event":"result","result":{"status":"ERROR","usage":{}}}"#]);
        assert_eq!(t.events(), [Event::Error { text: "agy finished with ERROR".into() }]);
    }

    #[test]
    fn file_edits_and_unknown_tools_become_their_own_events() {
        let t = transcribe(
            Agent::Agy,
            &[
                r#"{"event":"step_update","step_update":{"step_index":3,"step_type":"tool","tool_info":{"name":"write_to_file","parameters":{"TargetFile":"/w/a.rs"}}}}"#,
                r#"{"event":"step_update","step_update":{"step_index":4,"step_type":"tool","tool_info":{"name":"browser_get_dom","parameters":{"url":"http://x"}}}}"#,
            ],
        );
        assert_eq!(
            t.events(),
            [
                Event::Edit { path: "/w/a.rs".into() },
                Event::Tool { name: "browser_get_dom".into(), summary: "http://x".into() }
            ]
        );
    }
}
