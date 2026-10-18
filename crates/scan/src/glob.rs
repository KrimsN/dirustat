//! Hand-written wildcard matcher (`*` any run, `?` any one character) shared by scan exclusions
//! and the engine's search. No regex engine, no recursion: the classic two-pointer walk that
//! remembers only the last `*`, so there is no exponential backtracking, only O(pattern * text)
//! in the worst case. Runs of `*` are collapsed when the pattern is compiled.

use std::fmt;

/// Longest accepted pattern, in characters.
pub const MAX_PATTERN_CHARS: usize = 1024;

/// Why a pattern was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobError {
    Empty,
    TooLong,
    /// A NUL can never occur in a file name, so such a pattern is a mistake or hostile.
    Nul,
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("pattern is empty"),
            Self::TooLong => write!(f, "pattern is longer than {MAX_PATTERN_CHARS} characters"),
            Self::Nul => f.write_str("pattern contains a NUL character"),
        }
    }
}

impl std::error::Error for GlobError {}

/// A compiled wildcard pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    pattern: Vec<char>,
    case_insensitive: bool,
}

/// Simple (non-locale) case folding: lowercase every character.
fn fold(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase()
}

impl Glob {
    /// Compiles `pattern`. With `case_insensitive`, both sides are lowercased character by
    /// character (Unicode-safe, no locale rules).
    pub fn new(pattern: &str, case_insensitive: bool) -> Result<Self, GlobError> {
        if pattern.is_empty() {
            return Err(GlobError::Empty);
        }
        if pattern.contains('\0') {
            return Err(GlobError::Nul);
        }
        if pattern.chars().take(MAX_PATTERN_CHARS.saturating_add(1)).count() > MAX_PATTERN_CHARS {
            return Err(GlobError::TooLong);
        }
        let mut compiled: Vec<char> = Vec::new();
        for c in pattern.chars() {
            if c == '*' && compiled.last() == Some(&'*') {
                continue;
            }
            if case_insensitive && c != '*' && c != '?' {
                compiled.extend(fold(c));
            } else {
                compiled.push(c);
            }
        }
        Ok(Self { pattern: compiled, case_insensitive })
    }

    /// Whether the whole of `text` matches.
    pub fn is_match(&self, text: &str) -> bool {
        let text: Vec<char> =
            if self.case_insensitive { text.chars().flat_map(fold).collect() } else { text.chars().collect() };
        match_chars(&self.pattern, &text)
    }
}

fn match_chars(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    // After the last `*`: where the pattern resumes and the text position to retry from.
    let mut star: Option<(usize, usize)> = None;
    loop {
        let Some(&tc) = text.get(t) else {
            while pattern.get(p) == Some(&'*') {
                p = p.saturating_add(1);
            }
            return p >= pattern.len();
        };
        match pattern.get(p) {
            Some('*') => {
                p = p.saturating_add(1);
                star = Some((p, t));
            }
            Some(&pc) if pc == '?' || pc == tc => {
                p = p.saturating_add(1);
                t = t.saturating_add(1);
            }
            _ => {
                let Some((resume, from)) = star else { return false };
                let next = from.saturating_add(1);
                star = Some((resume, next));
                p = resume;
                t = next;
            }
        }
    }
}

/// A set of exclusion patterns for scans.
///
/// A pattern without a path separator is matched against the entry name; a pattern containing `/`
/// or `\` is matched against the whole path (separators normalized to `/`). Matching is
/// case-insensitive on Windows and case-sensitive elsewhere.
#[derive(Clone, Debug, Default)]
pub struct Excludes {
    names: Vec<Glob>,
    paths: Vec<Glob>,
}

/// Most exclusion patterns a set holds.
pub const MAX_EXCLUDES: usize = 256;

/// Most characters all patterns of one set may hold together (every entry of a scan is matched
/// against every pattern, so the total length bounds the per-entry cost).
pub const MAX_EXCLUDES_TOTAL_CHARS: usize = 16 * 1024;

/// Whether the combined length of `patterns` stays within [`MAX_EXCLUDES_TOTAL_CHARS`].
pub fn excludes_total_len_ok(patterns: &[String]) -> bool {
    let mut total = 0usize;
    for p in patterns {
        total = total.saturating_add(p.chars().take(MAX_PATTERN_CHARS.saturating_add(1)).count());
        if total > MAX_EXCLUDES_TOTAL_CHARS {
            return false;
        }
    }
    true
}

impl Excludes {
    /// Compiles `patterns`; invalid ones (empty, too long, NUL) and those past
    /// [`MAX_EXCLUDES`] or past [`MAX_EXCLUDES_TOTAL_CHARS`] in total are dropped.
    pub fn new(patterns: &[String]) -> Self {
        let ci = cfg!(windows);
        let mut set = Self::default();
        let mut budget = MAX_EXCLUDES_TOTAL_CHARS;
        for raw in patterns.iter().take(MAX_EXCLUDES) {
            let len = raw.chars().take(MAX_PATTERN_CHARS.saturating_add(1)).count();
            let Some(left) = budget.checked_sub(len) else { break };
            budget = left;
            let has_sep = raw.contains(['/', '\\']);
            let normalized = raw.replace('\\', "/");
            let Ok(glob) = Glob::new(if has_sep { &normalized } else { raw }, ci) else { continue };
            if has_sep {
                set.paths.push(glob);
            } else {
                set.names.push(glob);
            }
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    /// Whether the entry called `name` at `path` is excluded.
    pub fn is_excluded(&self, name: &str, path: &std::path::Path) -> bool {
        if self.names.iter().any(|g| g.is_match(name)) {
            return true;
        }
        if self.paths.is_empty() {
            return false;
        }
        let full = path.to_string_lossy().replace('\\', "/");
        self.paths.iter().any(|g| g.is_match(&full))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn m(pattern: &str, text: &str) -> bool {
        Glob::new(pattern, false).unwrap().is_match(text)
    }

    #[test]
    fn basic_wildcards() {
        assert!(m("*.txt", "a.txt"));
        assert!(m("*.txt", ".txt"));
        assert!(!m("*.txt", "a.txt.bak"));
        assert!(m("a?c", "abc"));
        assert!(!m("a?c", "ac"));
        assert!(m("*", ""));
        assert!(m("*", "anything"));
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(!m("a*b*c", "aXXbYY"));
        assert!(m("literal", "literal"));
        assert!(!m("literal", "literals"));
        assert!(m("*a*a*a", "aaa"));
        assert!(!m("?", ""));
    }

    #[test]
    fn unicode_and_case() {
        assert!(m("файл_?.txt", "файл_日.txt"));
        assert!(m("*\u{1f980}*", "x\u{1f980}y"));
        let ci = Glob::new("*.TXT", true).unwrap();
        assert!(ci.is_match("Report.txt"));
        assert!(Glob::new("ПРИВЕТ*", true).unwrap().is_match("привет мир"));
        assert!(!Glob::new("*.TXT", false).unwrap().is_match("a.txt"));
    }

    #[test]
    fn invalid_patterns_are_errors() {
        assert_eq!(Glob::new("", false), Err(GlobError::Empty));
        assert_eq!(Glob::new("a\0b", false), Err(GlobError::Nul));
        assert_eq!(Glob::new(&"a".repeat(MAX_PATTERN_CHARS + 1), false), Err(GlobError::TooLong));
        assert!(Glob::new(&"a".repeat(MAX_PATTERN_CHARS), false).is_ok());
    }

    #[test]
    fn hostile_patterns_finish_quickly() {
        let start = std::time::Instant::now();
        let stars = format!("{}b", "*".repeat(1000));
        assert!(!m(&stars, &"a".repeat(100_000)));
        let alternating = format!("{}b", "a*".repeat(500));
        assert!(!m(&alternating, &"a".repeat(2000)));
        assert!(m(&"?".repeat(1000), &"x".repeat(1000)));
        assert!(!m(&"?".repeat(1000), &"x".repeat(999)));
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn excludes_match_names_and_paths() {
        let set = Excludes::new(&["node_modules".into(), "*.tmp".into(), "/proj/build/*".into(), String::new()]);
        assert!(!set.is_empty());
        assert!(set.is_excluded("node_modules", Path::new("/a/node_modules")));
        assert!(set.is_excluded("x.tmp", Path::new("/a/x.tmp")));
        assert!(!set.is_excluded("x.txt", Path::new("/a/x.txt")));
        assert!(set.is_excluded("out", Path::new("/proj/build/out")));
        assert!(!set.is_excluded("out", Path::new("/proj/src/out")));
        assert!(Excludes::new(&[]).is_empty());
        assert!(Excludes::new(&["a\0".into()]).is_empty());
        let many: Vec<String> = (0..MAX_EXCLUDES + 50).map(|i| format!("n{i}")).collect();
        assert_eq!(Excludes::new(&many).names.len(), MAX_EXCLUDES);
    }

    #[test]
    fn total_pattern_length_is_capped() {
        let long = "a".repeat(MAX_PATTERN_CHARS - 4);
        let many: Vec<String> = (0..MAX_EXCLUDES).map(|i| format!("{long}{i:04}")).collect();
        assert!(!excludes_total_len_ok(&many));
        let set = Excludes::new(&many);
        assert!(set.names.len() <= MAX_EXCLUDES_TOTAL_CHARS / MAX_PATTERN_CHARS);
        assert!(!set.is_empty());
        assert!(excludes_total_len_ok(&["*.tmp".to_owned(), "node_modules".to_owned()]));
    }
}
