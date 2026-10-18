//! Tells apart why an agent run failed, because each cause calls for a different reaction:
//! try another agent, wait and try again, or stop using the agent for this run.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The agent cannot be used: not signed in, forbidden, an option or model it does not know, or
    /// a quota that will not lift for hours.
    Unusable,
    /// The provider refused for rate or quota reasons, and will take requests again soon.
    Limited,
    /// Anything else; worth trying again.
    Transient,
    /// The user stopped it.
    Cancelled,
}

/// True when `code` appears as a number of its own, not inside a longer one.
fn has_code(text: &str, code: &str) -> bool {
    text.match_indices(code).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + code.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

/// A limit that will not lift for hours is no reason to wait and retry. Matches `resets in 134h7m2s`
/// and the plan-upgrade prompts that come with it.
fn out_of_quota_for_hours(lower: &str) -> bool {
    if any(lower, &["upgrade your subscription", "upgrade your plan"]) {
        return true;
    }
    lower.match_indices("resets in ").any(|(at, needle)| {
        let digits: String = lower[at + needle.len()..].chars().take_while(char::is_ascii_digit).collect();
        let after = &lower[at + needle.len() + digits.len()..];
        after.starts_with('h') && digits.parse::<u64>().is_ok_and(|hours| hours >= 1)
            || after.starts_with('d') && !digits.is_empty()
    })
}

fn unusable(lower: &str) -> bool {
    out_of_quota_for_hours(lower)
        || has_code(lower, "401")
        || has_code(lower, "403")
        || any(
            lower,
            &[
                "unauthorized",
                "unauthorised",
                "forbidden",
                "not logged in",
                "login required",
                "invalid model",
                "invalid api key",
                "unknown model",
                "unrecognized option",
                "unrecognized argument",
                "unexpected argument",
            ],
        )
        || (lower.contains("model") && any(lower, &["not found", "not available", "not supported"]))
}

fn limited(lower: &str) -> bool {
    has_code(lower, "429")
        || any(lower, &["rate limit", "rate-limit", "ratelimit", "quota", "usage limit", "overloaded", "exhausted"])
}

/// What went wrong, from the process's own words: its error output and the errors it reported.
pub fn classify(message: &str) -> Failure {
    let lower = message.to_lowercase();
    if unusable(&lower) {
        Failure::Unusable
    } else if limited(&lower) {
        Failure::Limited
    } else {
        Failure::Transient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_unknown_options_make_the_agent_unusable() {
        for message in
            ["HTTP 401 Unauthorized", "You are not logged in", "model gemini-9 not found", "unexpected argument '--x'"]
        {
            assert_eq!(classify(message), Failure::Unusable, "{message}");
        }
    }

    #[test]
    fn a_quota_that_resets_in_hours_makes_the_agent_unusable_but_one_that_resets_soon_is_only_a_limit() {
        for message in [
            "Individual quota reached. Please upgrade your subscription. Resets in 134h7m2s.",
            "Usage limit reached. Resets in 5h",
            "quota exceeded, resets in 2d",
        ] {
            assert_eq!(classify(message), Failure::Unusable, "{message}");
        }
        for message in ["Rate limit reached. Resets in 30s", "quota exceeded, resets in 45m", "429 Too Many Requests"] {
            assert_eq!(classify(message), Failure::Limited, "{message}");
        }
    }

    #[test]
    fn a_status_code_only_counts_as_a_number_of_its_own() {
        assert_eq!(classify("took 14290 ms"), Failure::Transient, "429 inside 14290 is not a rate limit");
        assert_eq!(classify("connection reset by peer"), Failure::Transient);
    }

    #[test]
    fn an_unusable_agent_wins_over_a_limit_in_the_same_message() {
        assert_eq!(classify("401 unauthorized, then quota"), Failure::Unusable);
    }
}
