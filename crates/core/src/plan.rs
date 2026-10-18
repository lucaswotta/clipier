//! The plan a captain writes for a Crew: pieces of work, who does each, what each may touch and
//! what each waits for. A plan is checked before anything starts, and one that breaks a rule is
//! turned back with the reasons.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::agents::Mode;
use crate::roles::Roles;
use crate::scope;

/// Most pieces a plan may have.
pub const MAX_PIECES: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piece {
    pub id: String,
    pub role: String,
    pub title: String,
    pub brief: String,
    /// The files a writing piece may change, as paths or globs.
    #[serde(default)]
    pub touches: Vec<String>,
    /// The pieces that must be done first.
    #[serde(default)]
    pub after: Vec<String>,
    /// For a review: the piece it looks over.
    #[serde(default)]
    pub reviews: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub summary: String,
    pub pieces: Vec<Piece>,
}

/// Reads a plan out of a reply: the JSON may be fenced or surrounded by words.
pub fn parse(reply: &str) -> Result<Plan, String> {
    let start = reply.find('{').ok_or("the reply holds no JSON")?;
    let end = reply.rfind('}').ok_or("the reply holds no JSON")?;
    serde_json::from_str(&reply[start..=end]).map_err(|error| format!("the plan is not valid: {error}"))
}

/// Whether `from` waits, directly or not, for `target`.
fn waits_for(pieces: &HashMap<&str, &Piece>, from: &str, target: &str) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        for before in pieces.get(id).map(|p| p.after.as_slice()).unwrap_or_default() {
            if before == target {
                return true;
            }
            if seen.insert(before.as_str()) {
                stack.push(before);
            }
        }
    }
    false
}

fn has_cycle(pieces: &HashMap<&str, &Piece>) -> bool {
    pieces.keys().any(|id| waits_for(pieces, id, id))
}

/// Every reason the plan cannot be carried out, or `Ok` when there is none.
pub fn validate(plan: &Plan, roles: &Roles) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    if plan.pieces.is_empty() {
        problems.push("the plan has no pieces".to_owned());
    }
    if plan.pieces.len() > MAX_PIECES {
        problems.push(format!("the plan has {} pieces; the most is {MAX_PIECES}", plan.pieces.len()));
    }
    let mut by_id: HashMap<&str, &Piece> = HashMap::new();
    for piece in &plan.pieces {
        if by_id.insert(&piece.id, piece).is_some() {
            problems.push(format!("the id `{}` is used twice", piece.id));
        }
    }
    for piece in &plan.pieces {
        let Some(role) = roles.get(&piece.role) else {
            problems.push(format!("`{}` names the role `{}`, which does not exist", piece.id, piece.role));
            continue;
        };
        for before in &piece.after {
            if !by_id.contains_key(before.as_str()) {
                problems.push(format!("`{}` waits for `{before}`, which is not a piece", piece.id));
            }
        }
        for pattern in &piece.touches {
            if let Err(why) = scope::parse(pattern) {
                problems.push(format!("`{}`: {why}", piece.id));
            }
        }
        if role.mode == Mode::Read && !piece.touches.is_empty() {
            problems.push(format!("`{}` uses a read-only role but lists files it may change", piece.id));
        }
        if role.mode == Mode::Write && piece.touches.is_empty() {
            problems.push(format!("`{}` writes, so it must list the files it may change", piece.id));
        }
        if let Some(reviewed) = &piece.reviews {
            let target = by_id.get(reviewed.as_str());
            if role.mode == Mode::Write {
                problems.push(format!("`{}` reviews, so its role must be read-only", piece.id));
            }
            match target.and_then(|t| roles.get(&t.role)) {
                Some(target_role) if target_role.mode == Mode::Write => {
                    if !piece.after.contains(reviewed) {
                        problems.push(format!("`{}` reviews `{reviewed}` and must come after it", piece.id));
                    }
                }
                _ => problems.push(format!("`{}` reviews `{reviewed}`, which is not a writing piece", piece.id)),
            }
        }
    }
    if problems.is_empty() && has_cycle(&by_id) {
        problems.push("the pieces wait for each other in a circle".to_owned());
    }
    if problems.is_empty() {
        problems.extend(overlaps(plan, roles, &by_id));
    }
    if problems.is_empty() { Ok(()) } else { Err(problems) }
}

/// Two writing pieces that may run at the same time must not touch the same files.
fn overlaps(plan: &Plan, roles: &Roles, by_id: &HashMap<&str, &Piece>) -> Vec<String> {
    let writers: Vec<&Piece> =
        plan.pieces.iter().filter(|p| roles.get(&p.role).is_some_and(|r| r.mode == Mode::Write)).collect();
    let scopes =
        |piece: &Piece| piece.touches.iter().filter_map(|pattern| scope::parse(pattern).ok()).collect::<Vec<_>>();
    let mut problems = Vec::new();
    for (index, first) in writers.iter().enumerate() {
        for second in &writers[index + 1..] {
            let ordered = waits_for(by_id, &first.id, &second.id) || waits_for(by_id, &second.id, &first.id);
            if !ordered && scope::sets_overlap(&scopes(first), &scopes(second)) {
                problems.push(format!(
                    "`{}` and `{}` may run at the same time and touch the same files; make one wait for the other",
                    first.id, second.id
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roles::builtins;

    fn roles() -> Roles {
        Roles { list: builtins(), problems: Vec::new() }
    }

    fn piece(id: &str, role: &str, touches: &[&str], after: &[&str]) -> Piece {
        Piece {
            id: id.into(),
            role: role.into(),
            title: id.into(),
            brief: "do it".into(),
            touches: touches.iter().map(|t| (*t).to_owned()).collect(),
            after: after.iter().map(|a| (*a).to_owned()).collect(),
            reviews: None,
        }
    }

    fn plan(pieces: Vec<Piece>) -> Plan {
        Plan { summary: "s".into(), pieces }
    }

    fn problems(plan: &Plan) -> String {
        validate(plan, &roles()).unwrap_err().join(" | ")
    }

    #[test]
    fn a_plan_is_read_out_of_a_reply_that_fences_or_surrounds_it() {
        let reply = "Here is the plan:\n```json\n{\"summary\":\"two steps\",\"pieces\":[{\"id\":\"a\",\"role\":\"builder\",\"title\":\"A\",\"brief\":\"b\",\"touches\":[\"src/\"]}]}\n```\nDone.";
        let plan = parse(reply).unwrap();
        assert_eq!((plan.summary.as_str(), plan.pieces.len()), ("two steps", 1));
        assert_eq!(plan.pieces[0].after, Vec::<String>::new(), "missing lists are empty");
        assert!(parse("no json here").is_err());
        assert!(parse("{\"summary\": 3}").is_err());
    }

    #[test]
    fn a_good_plan_passes_and_pieces_that_wait_for_each_other_may_share_files() {
        let mut review = piece("look", "reviewer", &[], &["command"]);
        review.reviews = Some("command".into());
        let good = plan(vec![
            piece("schema", "builder", &["src/export/"], &[]),
            piece("command", "builder", &["src/export/command.rs", "src/cli/"], &["schema"]),
            review,
            piece("cli", "builder", &["docs/"], &[]),
        ]);
        assert_eq!(validate(&good, &roles()), Ok(()));
    }

    #[test]
    fn every_rule_is_checked_and_says_which_piece_breaks_it() {
        assert!(problems(&plan(vec![])).contains("no pieces"));
        assert!(
            problems(&plan(vec![piece("a", "builder", &["x"], &[]), piece("a", "builder", &["y"], &[])]))
                .contains("used twice")
        );
        assert!(problems(&plan(vec![piece("a", "wizard", &["x"], &[])])).contains("does not exist"));
        assert!(problems(&plan(vec![piece("a", "builder", &["x"], &["ghost"])])).contains("not a piece"));
        assert!(problems(&plan(vec![piece("a", "builder", &[], &[])])).contains("must list the files"));
        assert!(problems(&plan(vec![piece("a", "reviewer", &["x"], &[])])).contains("read-only role"));
        assert!(problems(&plan(vec![piece("a", "builder", &["../x"], &[])])).contains("must not contain"));
        let too_many = (0..13).map(|i| piece(&format!("p{i}"), "builder", &[&format!("d{i}/")], &[])).collect();
        assert!(problems(&plan(too_many)).contains("the most is 12"));
    }

    #[test]
    fn a_circle_of_waiting_and_a_clash_between_parallel_pieces_are_turned_back() {
        let circle = plan(vec![piece("a", "builder", &["x/"], &["b"]), piece("b", "builder", &["y/"], &["a"])]);
        assert!(problems(&circle).contains("in a circle"));
        let clash = plan(vec![piece("a", "builder", &["src/"], &[]), piece("b", "builder", &["src/lib.rs"], &[])]);
        assert!(problems(&clash).contains("at the same time"));
    }

    #[test]
    fn a_review_must_name_a_writing_piece_and_come_after_it() {
        let mut stray = piece("look", "reviewer", &[], &[]);
        stray.reviews = Some("a".into());
        assert!(problems(&plan(vec![piece("a", "builder", &["x/"], &[]), stray])).contains("must come after"));
        let mut of_reader = piece("look", "reviewer", &[], &["scan"]);
        of_reader.reviews = Some("scan".into());
        assert!(problems(&plan(vec![piece("scan", "tester", &[], &[]), of_reader])).contains("not a writing piece"));
    }
}
