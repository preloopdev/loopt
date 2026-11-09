//! Validation of GitHub Actions filter patterns.
//!
//! The accepted syntax is the one described by GitHub's filter pattern cheat
//! sheet:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#filter-pattern-cheat-sheet>
//!
//! This mirrors actionlint's `glob.go` (`ValidateRefGlob` / `ValidatePathGlob`)
//! without its cosmetic "single-character class is useless" suggestion: only
//! patterns GitHub cannot interpret are rejected.

/// Validate a `branches`, `branches-ignore`, `tags`, or `tags-ignore` pattern.
pub(crate) fn validate_ref_glob(pattern: &str) -> Option<String> {
    validate(pattern, true).err()
}

/// Validate a `paths` or `paths-ignore` pattern.
pub(crate) fn validate_path_glob(pattern: &str) -> Option<String> {
    if pattern.trim().len() != pattern.len() {
        return Some("path patterns cannot have leading or trailing whitespace".to_owned());
    }
    let rest = pattern.strip_prefix('!').unwrap_or(pattern);
    if rest == "." || rest == ".." || rest.starts_with("./") || rest.starts_with("../") {
        return Some("path patterns cannot use `.` or `..` segments".to_owned());
    }
    validate(pattern, false).err()
}

fn validate(pattern: &str, is_ref: bool) -> Result<(), String> {
    let chars: Vec<char> = pattern.chars().collect();
    if chars.is_empty() {
        return Err("filter pattern cannot be empty".to_owned());
    }

    let mut index = 0;
    // Whether the previous element can be repeated by `?` or `+`. Nothing
    // precedes the first element, so a leading quantifier is invalid.
    let mut repeatable = false;

    if chars[0] == '!' {
        if chars.len() == 1 {
            return Err("`!` must be followed by at least one character".to_owned());
        }
        index = 1;
        repeatable = false;
    } else if is_ref && chars[0] == '/' {
        return Err("branch and tag patterns cannot start with `/`".to_owned());
    }

    while index < chars.len() {
        let character = chars[index];
        match character {
            '\\' => {
                let Some(&escaped) = chars.get(index + 1) else {
                    if is_ref {
                        return Err(
                            "`\\` must escape one of `[`, `?`, `+`, `*`, `\\`, `!`".to_owned()
                        );
                    }
                    index += 1;
                    repeatable = true;
                    continue;
                };
                if matches!(escaped, '[' | '?' | '*' | '+' | '\\' | '!') {
                    index += 2;
                    repeatable = true;
                } else if is_ref {
                    return Err(format!(
                        "`\\` can only escape `[`, `?`, `+`, `*`, `\\`, `!`; found `\\{escaped}`"
                    ));
                } else {
                    // Path patterns treat a backslash before anything else as a
                    // literal character.
                    index += 1;
                    repeatable = true;
                }
            }
            '?' | '+' => {
                if !repeatable {
                    return Err(format!(
                        "`{character}` must follow a character, not another special character"
                    ));
                }
                index += 1;
                repeatable = false;
            }
            '*' => {
                index += 1;
                repeatable = false;
            }
            '[' => {
                index += 1;
                if chars.get(index) == Some(&']') {
                    return Err("character class `[]` cannot be empty".to_owned());
                }
                loop {
                    let Some(&start) = chars.get(index) else {
                        return Err("character class is missing its closing `]`".to_owned());
                    };
                    if start == ']' {
                        index += 1;
                        break;
                    }
                    if chars.get(index + 1) == Some(&'-') {
                        let Some(&end) = chars.get(index + 2) else {
                            return Err("character range is missing its end".to_owned());
                        };
                        if end == ']' {
                            return Err("character range is missing its end".to_owned());
                        }
                        if start > end {
                            return Err(format!("character range `{start}-{end}` is reversed"));
                        }
                        index += 3;
                    } else {
                        index += 1;
                    }
                }
                repeatable = true;
            }
            '\n' | '\r' => {
                return Err("filter patterns cannot contain newlines".to_owned());
            }
            ' ' | '\t' | '~' | '^' | ':' if is_ref => {
                return Err(format!(
                    "`{character}` is not valid in a branch or tag pattern"
                ));
            }
            _ => {
                index += 1;
                repeatable = true;
            }
        }
    }

    if is_ref && matches!(chars.last(), Some('/' | '.')) {
        return Err("branch and tag patterns cannot end with `/` or `.`".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_path_glob, validate_ref_glob};

    #[test]
    fn accepts_common_ref_patterns() {
        for pattern in [
            "main",
            "releases/**",
            "releases/**-alpha",
            "v*",
            "**[0-9]+.[0-9]+.[0-9]+*",
            "!releases/**",
            "release/**/[0-9a-f]?",
            "tags/v1.*",
            "feature/*",
        ] {
            assert_eq!(validate_ref_glob(pattern), None, "{pattern}");
        }
    }

    #[test]
    fn rejects_malformed_ref_patterns() {
        for pattern in [
            "", "[0-9", "[]", "[0-]", "[z-a]", "foo bar", "foo:bar", "foo~", "/main", "main/",
            "main.", "foo\\q", "foo\\", "?foo", "*?",
        ] {
            assert!(
                validate_ref_glob(pattern).is_some(),
                "{pattern} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_common_path_patterns() {
        for pattern in [
            "**.md",
            "crates/**",
            ".github/workflows/**",
            "src/*.rs",
            "!docs/**",
            "foo\\bar",
        ] {
            assert_eq!(validate_path_glob(pattern), None, "{pattern}");
        }
    }

    #[test]
    fn rejects_malformed_path_patterns() {
        for pattern in [
            "", " docs/**", "docs/** ", ".", "..", "./src", "../src", "src/[abc", "src/[]",
            "src/[0-]",
        ] {
            assert!(
                validate_path_glob(pattern).is_some(),
                "{pattern} should be rejected"
            );
        }
    }
}
