//! Random identifiers.

/// A random version 4 UUID, for the CLIs that accept the session id up front.
pub fn uuid() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system provides random bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

const WINDS: [&str; 12] =
    ["steady", "fair", "brisk", "gentle", "bold", "swift", "quiet", "silver", "northern", "salty", "bright", "lucky"];
const SHIPS: [&str; 12] = [
    "albatross",
    "heron",
    "kestrel",
    "petrel",
    "tern",
    "gull",
    "osprey",
    "dolphin",
    "kraken",
    "clipper",
    "sloop",
    "brig",
];

/// A name for a run, like `fair-albatross-3f2a`. It becomes part of a branch name and of a folder.
pub fn run_name() -> String {
    let id = uuid();
    let pick = |from: &[&'static str], at: usize| {
        let byte = u8::from_str_radix(&id[at..at + 2], 16).unwrap_or(0);
        from[usize::from(byte) % from.len()]
    };
    format!("{}-{}-{}", pick(&WINDS, 0), pick(&SHIPS, 2), &id[4..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_has_the_shape_the_cli_accepts_and_is_not_repeated() {
        let (a, b) = (uuid(), uuid());
        assert_ne!(a, b);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4') && matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')));
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_run_name_is_a_valid_part_of_a_branch_name_and_is_not_repeated() {
        let (a, b) = (run_name(), run_name());
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{a}");
        assert_eq!(a.split('-').count(), 3);
    }
}
