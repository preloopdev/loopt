//! Naming rules shared by the TypeScript renderer.
//!
//! Schema definition names and property keys use kebab-case and snake_case.
//! The generated TypeScript uses PascalCase for type names and camelCase for
//! property names, so both spellings are derived by splitting on `-` and `_`.

/// Converts a schema definition name into its PascalCase TypeScript name.
///
/// Every `-` or `_` separated word has its first character upper-cased; the
/// rest of the word is left untouched.
pub(crate) fn pascal_case(name: &str) -> String {
    let mut out = String::new();
    for word in name.split(['-', '_']) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Converts an exact schema property key into its camelCase spelling.
///
/// The first `-` or `_` separated word has its first character lower-cased;
/// later words have theirs upper-cased. The remaining characters are left
/// untouched.
pub(crate) fn camel_case(name: &str) -> String {
    let mut out = String::new();
    for (index, word) in name.split(['-', '_']).enumerate() {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                out.extend(first.to_lowercase());
            } else {
                out.extend(first.to_uppercase());
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Whether `name` is an ASCII TypeScript identifier and may therefore be
/// emitted without quotes.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' || first == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}
