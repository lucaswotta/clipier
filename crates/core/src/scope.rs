//! Write scopes: which files a piece may change, and whether two scopes can touch the same file.
//!
//! The check is deliberately conservative. A glob is treated as "everything under its fixed
//! prefix", so two scopes may be reported as overlapping when they would not actually collide,
//! but never the other way round. Serializing too much costs time; missing a collision costs a
//! merge conflict.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One exact file.
    File(Vec<String>),
    /// Everything under a directory prefix (a trailing slash, a glob, or the whole repository).
    Under(Vec<String>),
}

const GLOB_CHARS: [char; 4] = ['*', '?', '[', '{'];

pub fn parse(pattern: &str) -> Result<Scope, String> {
    let pattern = pattern.trim();
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if pattern.is_empty() {
        return Err("empty path".into());
    }
    if pattern.starts_with('/') || pattern.contains('\\') {
        return Err(format!("`{pattern}` must be a relative path with forward slashes"));
    }
    let directory = pattern.ends_with('/');
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == ".." || *s == ".") {
        return Err(format!("`{pattern}` must not contain `.` or `..` segments"));
    }
    let fixed: Vec<String> = segments.iter().take_while(|s| !s.contains(GLOB_CHARS)).map(|s| (*s).to_owned()).collect();
    let has_glob = fixed.len() < segments.len();
    Ok(if has_glob || directory { Scope::Under(fixed) } else { Scope::File(fixed) })
}

fn overlaps(a: &Scope, b: &Scope) -> bool {
    match (a, b) {
        (Scope::File(x), Scope::File(y)) => x == y,
        (Scope::File(file), Scope::Under(prefix)) | (Scope::Under(prefix), Scope::File(file)) => {
            file.starts_with(prefix)
        }
        (Scope::Under(x), Scope::Under(y)) => x.starts_with(y) || y.starts_with(x),
    }
}

pub fn sets_overlap(a: &[Scope], b: &[Scope]) -> bool {
    a.iter().any(|x| b.iter().any(|y| overlaps(x, y)))
}

/// Whether `path` (relative, forward slashes) is inside the scope written as `pattern`.
///
/// Supports `*` and `?` inside a segment and `**` as a whole segment. A pattern with other glob
/// syntax (`[` or `{`) falls back to its fixed prefix, which accepts more than it should; that is
/// the safe direction for conflict detection but the lenient one here, so such patterns should
/// be avoided in plans.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim();
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let path = path.strip_prefix("./").unwrap_or(path);
    let path_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut pattern_segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if pattern.ends_with('/') {
        pattern_segments.push("**");
    }
    if pattern_segments.iter().any(|s| s.contains(['[', '{'])) {
        return match parse(pattern) {
            Ok(Scope::File(fixed)) => path_segments == fixed.iter().map(String::as_str).collect::<Vec<_>>(),
            Ok(Scope::Under(prefix)) => {
                path_segments.starts_with(&prefix.iter().map(String::as_str).collect::<Vec<_>>())
            }
            Err(_) => false,
        };
    }
    segments_match(&pattern_segments, &path_segments)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skipped| segments_match(rest, &path[skipped..])),
        Some((segment, rest)) => {
            path.split_first().is_some_and(|(name, tail)| segment_matches(segment, name) && segments_match(rest, tail))
        }
    }
}

fn segment_matches(pattern: &str, name: &str) -> bool {
    fn go(pattern: &[char], name: &[char]) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len()).any(|skipped| go(rest, &name[skipped..])),
            Some(('?', rest)) => name.split_first().is_some_and(|(_, tail)| go(rest, tail)),
            Some((c, rest)) => name.split_first().is_some_and(|(n, tail)| n == c && go(rest, tail)),
        }
    }
    go(&pattern.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(pattern: &str) -> Scope {
        parse(pattern).unwrap()
    }

    fn collide(a: &str, b: &str) -> bool {
        overlaps(&scope(a), &scope(b))
    }

    #[test]
    fn files_collide_only_when_equal() {
        assert!(collide("src/lib.rs", "src/lib.rs"));
        assert!(collide("src/lib.rs", "./src/lib.rs"));
        assert!(!collide("src/lib.rs", "src/main.rs"));
    }

    #[test]
    fn a_directory_covers_the_files_under_it() {
        assert!(collide("src/", "src/lib.rs"));
        assert!(collide("src/lib.rs", "src/"));
        assert!(!collide("src/", "tests/a.rs"));
        assert!(!collide("src/", "srcs/a.rs"), "a prefix is matched per path segment, not per character");
    }

    #[test]
    fn globs_are_treated_as_their_fixed_prefix() {
        assert!(collide("src/**", "src/deep/x.rs"));
        assert!(collide("src/*.rs", "src/lib.rs"));
        assert!(collide("src/*.rs", "src/sub/"), "conservative: a glob may cover the subdirectory");
        assert!(!collide("src/**", "docs/**"));
    }

    #[test]
    fn nested_prefixes_collide_in_both_directions() {
        assert!(collide("a/b/", "a/"));
        assert!(collide("a/", "a/b/c/**"));
        assert!(!collide("a/b/", "a/c/"));
    }

    #[test]
    fn the_whole_repository_collides_with_everything() {
        assert!(collide("**", "src/lib.rs"));
        assert!(collide("*.md", "README.md"), "a root glob has an empty fixed prefix");
    }

    #[test]
    fn rejects_paths_that_escape_or_are_not_relative() {
        for bad in ["", "  ", "/etc/passwd", "../x", "a/../b", "a\\b"] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn set_overlap_checks_every_pair() {
        let a = [scope("src/a.rs"), scope("docs/")];
        let b = [scope("tests/"), scope("docs/guide.md")];
        assert!(sets_overlap(&a, &b));
        assert!(!sets_overlap(&a, &[scope("tests/")]));
        assert!(!sets_overlap(&[], &b));
    }

    #[test]
    fn matching_a_path_against_a_scope() {
        assert!(matches("src/lib.rs", "src/lib.rs"));
        assert!(!matches("src/lib.rs", "src/main.rs"));
        assert!(matches("src/", "src/a/b.rs"));
        assert!(!matches("src/", "srcs/a.rs"));
        assert!(matches("src/**", "src/a/b.rs"));
        assert!(matches("src/**", "src"), "`**` also matches nothing");
        assert!(matches("src/*.rs", "src/lib.rs"));
        assert!(!matches("src/*.rs", "src/deep/lib.rs"), "`*` does not cross a slash");
        assert!(matches("src/**/mod.rs", "src/a/b/mod.rs"));
        assert!(matches("src/**/mod.rs", "src/mod.rs"));
        assert!(matches("a?c.txt", "abc.txt"));
        assert!(!matches("a?c.txt", "ac.txt"));
        assert!(matches("./docs/readme.md", "docs/readme.md"));
        assert!(!matches("docs/readme.md", "docs"), "a file scope does not cover its parent");
    }
}
