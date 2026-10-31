//! Generic workflow authoring without execution-time schema validation.
//!
//! The JSON view retains fields, declaration order and scalar types. Scalar
//! mapping keys normalize to strings (null becomes "null"); collisions fail.
//! YAML aliases are resolved and `<<` merges are expanded (explicit fields win;
//! earlier merge-sequence entries win). Custom tags, complex mapping keys and
//! non-finite numbers have no faithful JSON representation and are rejected.
//! Merge-shaped keys whose scalar spelling cannot be classified safely are also
//! rejected rather than silently treating a literal field as a merge.
//! The original source is returned byte-for-byte until the document changes.
//! Edits first attempt verified, source-local scalar replacements, then verified
//! top-level field replacements, additions, removals and reordering. Unchanged
//! root fields retain their source text; regenerated regions can lose comments
//! and anchor spelling. This is not a general-purpose lossless YAML editor.

use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

/// A syntax or representation error in an authoring document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentError {
    /// Human-readable explanation of the failure.
    pub message: String,
    /// One-based source line, when available.
    pub line: Option<usize>,
    /// One-based source column, when available.
    pub column: Option<usize>,
    /// JSON Pointer identifying the offending value, when available.
    pub path: Option<String>,
}

impl DocumentError {
    fn at(message: impl Into<String>, path: &str) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
            path: Some(path.to_owned()),
        }
    }

    fn yaml(error: serde_yaml::Error) -> Self {
        let location = error.location();
        Self {
            message: error.to_string(),
            line: location.as_ref().map(|location| location.line()),
            column: location.as_ref().map(|location| location.column()),
            path: None,
        }
    }
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(path) = &self.path {
            write!(
                formatter,
                " at {}",
                if path.is_empty() { "/" } else { path }
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for DocumentError {}

/// A schema-independent, ordered GitHub Actions authoring document.
///
/// This is not an expanded job plan. Workflow and action metadata mappings,
/// unknown fields and intentionally execution-invalid documents are accepted.
/// Source-backed no-op serialization is exact. Scalar edits attempt verified
/// token replacements; structural edits may regenerate whole top-level fields.
/// If neither is safe, the entire document is normalized, losing source comments
/// and anchor spelling. No losslessness is promised for changed regions.
#[derive(Clone, Debug)]
pub struct WorkflowDocument {
    value: Value,
    source: String,
}

impl WorkflowDocument {
    /// Parse exactly one YAML mapping without runtime workflow validation.
    pub fn parse(source: &str) -> Result<Self, DocumentError> {
        let value = parse_value(source)?;
        Ok(Self {
            value,
            source: source.to_owned(),
        })
    }

    /// Create a document from an ordered JSON mapping.
    ///
    /// The emitted YAML is reparsed before acceptance to ensure that conversion
    /// preserves the complete JSON value, including nested declaration order.
    pub fn from_value(value: Value) -> Result<Self, DocumentError> {
        let source = render(&value)?;
        Ok(Self { value, source })
    }

    /// Borrow the unexpanded, ordered JSON authoring view.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Borrow the original YAML source.
    ///
    /// Static rules that need comment or formatting provenance (for example
    /// whether a permission grant is explained) read this instead of guessing
    /// from the JSON view.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Emit the current document as YAML.
    ///
    /// Unchanged source-backed documents are emitted byte-for-byte. Changed
    /// documents preserve source trivia only outside regenerated regions.
    pub fn to_yaml(&self) -> Result<String, DocumentError> {
        Ok(self.source.clone())
    }

    /// Transactionally replace the complete JSON authoring view.
    ///
    /// Validation and source regeneration finish before either stored value is
    /// modified. Scalar edits preserve surrounding source when a complete
    /// reparse proves equivalence; structural edits can normalize larger regions.
    pub fn replace(&mut self, value: Value) -> Result<(), DocumentError> {
        if ordered_eq(&self.value, &value) {
            return Ok(());
        }
        let source = match patch_scalars(&self.source, &self.value, &value)
            .or_else(|| patch_root_fields(&self.source, &self.value, &value))
        {
            Some(source) => source,
            None => render(&value)?,
        };
        self.value = value;
        self.source = source;
        Ok(())
    }
}

fn pointer(parent: &str, segment: &str) -> String {
    format!("{parent}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

fn parse_value(source: &str) -> Result<Value, DocumentError> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(source).map_err(DocumentError::yaml)?;
    let (yaml, literal_merge_key) = protect_literal_merge_keys(source, yaml)?;
    let yaml = resolve_merges(yaml, "")?;
    let value = json_value(yaml, "", literal_merge_key.as_deref())?;
    if !value.is_object() {
        return Err(DocumentError::at(
            "authoring document must be a mapping",
            "",
        ));
    }
    Ok(value)
}

// serde_yaml::Value erases the distinction between a plain merge key and a
// quoted literal "<<" key. Protect quoted keys before conversion. Token
// discovery alone is never authoritative: renaming the parsed candidate back
// must reproduce the complete original YAML tree before a token is accepted.
fn protect_literal_merge_keys(
    source: &str,
    original: serde_yaml::Value,
) -> Result<(serde_yaml::Value, Option<String>), DocumentError> {
    if !has_yaml_key(&original, "<<") {
        return Ok((original, None));
    }
    let mut marker = "__preloop_literal_merge_key__".to_owned();
    while has_yaml_key(&original, &marker) {
        marker.push('_');
    }
    let replacement =
        serde_json::to_string(&marker).map_err(|error| DocumentError::at(error.to_string(), ""))?;
    let mut ranges = Vec::new();
    let bytes = source.as_bytes();
    // A YAML scalar can only begin at the start of the document/line, after
    // `{`/`,`/`?` in flow context, or after other whitespace (block indent,
    // `- ` sequence markers, `: ` values). Restricting candidate starts to
    // those positions is what keeps this a single effective pass: any run of
    // adjacent quote characters (doubled escapes, repeated literal quotes)
    // only ever offers one valid start, so no byte is rescanned as the head
    // of more than one forward probe below.
    for (start, quote) in bytes.iter().copied().enumerate() {
        if quote != b'\'' && quote != b'"' {
            continue;
        }
        if start != 0
            && !bytes[start - 1].is_ascii_whitespace()
            && !b"{,?".contains(&bytes[start - 1])
        {
            continue;
        }
        let mut end = start + 1;
        while end < bytes.len() {
            if quote == b'"' && bytes[end] == b'\\' {
                end += 2;
                continue;
            }
            if bytes[end] == quote {
                if quote == b'\'' && bytes.get(end + 1) == Some(&quote) {
                    end += 2;
                    continue;
                }
                break;
            }
            end += 1;
        }
        if end >= bytes.len() || !source[end + 1..].trim_start().starts_with(':') {
            continue;
        }
        if serde_yaml::from_str::<String>(&source[start..=end])
            .ok()
            .as_deref()
            != Some("<<")
        {
            continue;
        }
        let range = start..end + 1;
        let mut candidate = source.to_owned();
        candidate.replace_range(range.clone(), &replacement);
        let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(&candidate) else {
            continue;
        };
        if rename_yaml_keys(parsed, &marker, "<<") == original {
            ranges.push(range);
        }
    }
    // Explicit core string tags also make an unquoted << a literal key.
    for tag in ["!!str", "!<tag:yaml.org,2002:str>"] {
        for (start, _) in source.match_indices(tag) {
            let after_tag = &source[start + tag.len()..];
            let scalar = after_tag.trim_start();
            if !scalar.starts_with("<<") || !scalar[2..].trim_start().starts_with(':') {
                continue;
            }
            let end = start + tag.len() + after_tag.len() - scalar.len() + 2;
            let range = start..end;
            let mut candidate = source.to_owned();
            candidate.replace_range(range.clone(), &replacement);
            let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(&candidate) else {
                continue;
            };
            if rename_yaml_keys(parsed, &marker, "<<") == original {
                ranges.push(range);
            }
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    if ranges.is_empty() {
        validate_plain_merge_keys(source, &original)?;
        return Ok((original, None));
    }
    let mut protected = source.to_owned();
    for range in ranges.into_iter().rev() {
        protected.replace_range(range, &replacement);
    }
    let parsed = serde_yaml::from_str(&protected).map_err(DocumentError::yaml)?;
    validate_plain_merge_keys(&protected, &parsed)?;
    Ok((parsed, Some(marker)))
}

fn validate_plain_merge_keys(
    source: &str,
    original: &serde_yaml::Value,
) -> Result<(), DocumentError> {
    if !has_yaml_key(original, "<<") {
        return Ok(());
    }
    let mut marker = "__preloop_plain_merge_key__".to_owned();
    while has_yaml_key(original, &marker) {
        marker.push('_');
    }
    let mut ranges = Vec::new();
    for (start, _) in source.match_indices("<<") {
        if start != 0
            && !source.as_bytes()[start - 1].is_ascii_whitespace()
            && !b"{,?".contains(&source.as_bytes()[start - 1])
        {
            continue;
        }
        if !source[start + 2..]
            .trim_start_matches([' ', '\t'])
            .starts_with(':')
        {
            continue;
        }
        let range = start..start + 2;
        let mut candidate = source.to_owned();
        candidate.replace_range(range.clone(), &marker);
        let Ok(parsed) = serde_yaml::from_str::<serde_yaml::Value>(&candidate) else {
            continue;
        };
        if rename_yaml_keys(parsed, &marker, "<<") == *original {
            ranges.push(range);
        }
    }
    let mut classified = source.to_owned();
    for range in ranges.into_iter().rev() {
        classified.replace_range(range, &marker);
    }
    let classified: serde_yaml::Value =
        serde_yaml::from_str(&classified).map_err(DocumentError::yaml)?;
    if has_yaml_key(&classified, "<<") {
        return Err(DocumentError::at(
            "merge-shaped YAML key spelling cannot be represented faithfully; use a quoted literal key or an inline plain <<: merge key",
            "",
        ));
    }
    Ok(())
}

fn has_yaml_key(value: &serde_yaml::Value, key: &str) -> bool {
    match value {
        serde_yaml::Value::Mapping(mapping) => mapping.iter().any(|(candidate, value)| {
            candidate.as_str() == Some(key)
                || has_yaml_key(candidate, key)
                || has_yaml_key(value, key)
        }),
        serde_yaml::Value::Sequence(values) => values.iter().any(|value| has_yaml_key(value, key)),
        serde_yaml::Value::Tagged(tagged) => has_yaml_key(&tagged.value, key),
        _ => false,
    }
}

fn rename_yaml_keys(value: serde_yaml::Value, from: &str, to: &str) -> serde_yaml::Value {
    use serde_yaml::Value as Yaml;
    match value {
        Yaml::Mapping(mapping) => Yaml::Mapping(
            mapping
                .into_iter()
                .map(|(key, value)| {
                    let key = if key.as_str() == Some(from) {
                        Yaml::String(to.to_owned())
                    } else {
                        rename_yaml_keys(key, from, to)
                    };
                    (key, rename_yaml_keys(value, from, to))
                })
                .collect(),
        ),
        Yaml::Sequence(values) => Yaml::Sequence(
            values
                .into_iter()
                .map(|value| rename_yaml_keys(value, from, to))
                .collect(),
        ),
        Yaml::Tagged(mut tagged) => {
            tagged.value = rename_yaml_keys(tagged.value, from, to);
            Yaml::Tagged(tagged)
        }
        value => value,
    }
}

// Expand YAML merges before scalar keys are normalized to JSON strings. This
// preserves raw YAML key identity long enough to distinguish a true override
// (`1` merged with explicit `1`) from a collision (`1` merged with explicit
// `'1'`), which must not be silently discarded by merge precedence.
fn resolve_merges(
    value: serde_yaml::Value,
    path: &str,
) -> Result<serde_yaml::Value, DocumentError> {
    use serde_yaml::Value as Yaml;
    match value {
        Yaml::Sequence(values) => Ok(Yaml::Sequence(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| resolve_merges(value, &pointer(path, &index.to_string())))
                .collect::<Result<_, _>>()?,
        )),
        Yaml::Mapping(mapping) => {
            let explicit_keys: Vec<_> = mapping
                .keys()
                .filter(|key| key.as_str() != Some("<<"))
                .cloned()
                .collect();
            let entries = mapping
                .into_iter()
                .map(|(key, value)| {
                    let child_path = pointer(path, key.as_str().unwrap_or(""));
                    Ok::<_, DocumentError>((key, resolve_merges(value, &child_path)?))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut resolved = serde_yaml::Mapping::with_capacity(entries.len());
            for (key, value) in entries {
                if key.as_str() != Some("<<") {
                    resolved.insert(key, value);
                    continue;
                }
                let merge_path = pointer(path, "<<");
                match value {
                    Yaml::Mapping(merge) => {
                        add_merge_entries(&mut resolved, &explicit_keys, merge);
                    }
                    Yaml::Sequence(merges) => {
                        for merge in merges {
                            let Yaml::Mapping(merge) = merge else {
                                return Err(DocumentError::at(
                                    "YAML merge entries must be mappings",
                                    &merge_path,
                                ));
                            };
                            add_merge_entries(&mut resolved, &explicit_keys, merge);
                        }
                    }
                    _ => {
                        return Err(DocumentError::at(
                            "YAML merge value must be a mapping or sequence of mappings",
                            &merge_path,
                        ));
                    }
                }
            }
            Ok(Yaml::Mapping(resolved))
        }
        value => Ok(value),
    }
}

fn add_merge_entries(
    resolved: &mut serde_yaml::Mapping,
    explicit_keys: &[serde_yaml::Value],
    merge: serde_yaml::Mapping,
) {
    for (key, value) in merge {
        // Explicit keys override only the same raw YAML key. A key with a
        // different YAML scalar type must survive until json_value normalizes
        // it, where the resulting collision is diagnosed.
        if explicit_keys.iter().any(|explicit| explicit == &key) || resolved.contains_key(&key) {
            continue;
        }
        resolved.insert(key, value);
    }
}

fn json_value(
    value: serde_yaml::Value,
    path: &str,
    literal_merge_key: Option<&str>,
) -> Result<Value, DocumentError> {
    use serde_yaml::Value as Yaml;
    Ok(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(value),
        Yaml::String(value) => Value::String(value),
        Yaml::Number(value) => {
            let number = if let Some(value) = value.as_i64() {
                value.into()
            } else if let Some(value) = value.as_u64() {
                value.into()
            } else {
                serde_json::Number::from_f64(value.as_f64().ok_or_else(|| {
                    DocumentError::at("YAML number cannot be represented in JSON", path)
                })?)
                .ok_or_else(|| DocumentError::at("non-finite YAML number is not JSON", path))?
            };
            Value::Number(number)
        }
        Yaml::Sequence(values) => Value::Array(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    json_value(value, &pointer(path, &index.to_string()), literal_merge_key)
                })
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Mapping(mapping) => {
            let mut explicit = HashSet::new();
            let mut object = Map::with_capacity(mapping.len());
            for (key, value) in mapping {
                let key = scalar_key(key, path)?;
                let key = if Some(key.as_str()) == literal_merge_key {
                    "<<".to_owned()
                } else {
                    key
                };
                if !explicit.insert(key.clone()) {
                    return Err(DocumentError::at(
                        "mapping keys collide after scalar normalization",
                        path,
                    ));
                }
                let child_path = pointer(path, &key);
                object.insert(key, json_value(value, &child_path, literal_merge_key)?);
            }
            Value::Object(object)
        }
        Yaml::Tagged(_) => {
            return Err(DocumentError::at(
                "custom YAML tags cannot be represented in JSON",
                path,
            ));
        }
    })
}

fn scalar_key(key: serde_yaml::Value, path: &str) -> Result<String, DocumentError> {
    use serde_yaml::Value as Yaml;
    match key {
        Yaml::String(key) => Ok(key),
        Yaml::Null => Ok("null".to_owned()),
        Yaml::Bool(key) => Ok(key.to_string()),
        Yaml::Number(key) if key.as_f64().is_some_and(f64::is_finite) => Ok(key.to_string()),
        Yaml::Number(_) => Err(DocumentError::at(
            "non-finite YAML mapping key is not JSON",
            path,
        )),
        Yaml::Tagged(_) => Err(DocumentError::at(
            "custom YAML tags cannot be represented in JSON",
            path,
        )),
        _ => Err(DocumentError::at(
            "complex YAML mapping key is not JSON",
            path,
        )),
    }
}

fn render(value: &Value) -> Result<String, DocumentError> {
    if !value.is_object() {
        return Err(DocumentError::at(
            "authoring document must be a mapping",
            "",
        ));
    }
    let source = emit_yaml(value)?;
    let reparsed = parse_value(&source)?;
    if !ordered_eq(value, &reparsed) {
        return Err(DocumentError::at(
            "JSON value cannot roundtrip through the YAML authoring representation",
            "",
        ));
    }
    Ok(source)
}

// serde_yaml emits a string key named << as a plain YAML merge directive.
// Serialize those keys through a collision-free plain marker, then quote only
// that marker. No user key or scalar can contain the selected marker.
fn emit_yaml(value: &Value) -> Result<String, DocumentError> {
    if !has_json_key(value, "<<") {
        return serde_yaml::to_string(value).map_err(DocumentError::yaml);
    }
    let mut marker = "__preloop_literal_merge_key__".to_owned();
    while contains_json_text(value, &marker) {
        marker.push('_');
    }
    let source = serde_yaml::to_string(&YamlValue {
        value,
        marker: &marker,
    })
    .map_err(DocumentError::yaml)?;
    Ok(source.replace(&marker, "'<<'"))
}

fn has_json_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(values) => {
            values.contains_key(key) || values.values().any(|value| has_json_key(value, key))
        }
        Value::Array(values) => values.iter().any(|value| has_json_key(value, key)),
        _ => false,
    }
}

fn contains_json_text(value: &Value, text: &str) -> bool {
    match value {
        Value::String(value) => value.contains(text),
        Value::Array(values) => values.iter().any(|value| contains_json_text(value, text)),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(text) || contains_json_text(value, text)),
        _ => false,
    }
}

struct YamlValue<'a> {
    value: &'a Value,
    marker: &'a str,
}

impl Serialize for YamlValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.value {
            Value::Array(values) => serializer.collect_seq(values.iter().map(|value| YamlValue {
                value,
                marker: self.marker,
            })),
            Value::Object(values) => serializer.collect_map(values.iter().map(|(key, value)| {
                (
                    if key == "<<" {
                        self.marker
                    } else {
                        key.as_str()
                    },
                    YamlValue {
                        value,
                        marker: self.marker,
                    },
                )
            })),
            value => value.serialize(serializer),
        }
    }
}

fn ordered_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((lk, lv), (rk, rv))| lk == rk && ordered_eq(lv, rv))
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| ordered_eq(left, right))
        }
        _ => left == right,
    }
}

fn equivalent(source: &str, expected: &Value) -> bool {
    parse_value(source).is_ok_and(|actual| ordered_eq(&actual, expected))
}

fn scalar_changes<'a>(
    old: &'a Value,
    new: &'a Value,
    path: &str,
    changes: &mut Vec<(String, &'a Value, &'a Value)>,
) -> bool {
    if ordered_eq(old, new) {
        return true;
    }
    match (old, new) {
        (Value::Object(old), Value::Object(new)) if old.keys().eq(new.keys()) => old
            .iter()
            .all(|(key, old)| scalar_changes(old, &new[key], &pointer(path, key), changes)),
        (Value::Array(old), Value::Array(new)) if old.len() == new.len() => {
            old.iter().zip(new).enumerate().all(|(index, (old, new))| {
                scalar_changes(old, new, &pointer(path, &index.to_string()), changes)
            })
        }
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => false,
        _ => {
            changes.push((path.to_owned(), old, new));
            true
        }
    }
}

fn patch_scalars(source: &str, old: &Value, new: &Value) -> Option<String> {
    let mut changes = Vec::new();
    if !scalar_changes(old, new, "", &mut changes) {
        return None;
    }
    let mut source = source.to_owned();
    let mut expected = old.clone();
    for (path, old, new) in changes {
        *expected.pointer_mut(&path)? = new.clone();
        // JSON scalar syntax is valid YAML and cannot accidentally introduce
        // an expression, alias, comment or implicit scalar-type conversion.
        let replacement = serde_json::to_string(new).ok()?;
        let mut found = None;
        for range in scalar_ranges(&source) {
            let token = &source[range.clone()];
            let scalar: serde_yaml::Value = match serde_yaml::from_str(token) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Ok(actual) = json_value(scalar, "", None) else {
                continue;
            };
            if !ordered_eq(&actual, old) {
                continue;
            }
            let mut candidate = source.clone();
            candidate.replace_range(range, &replacement);
            if equivalent(&candidate, &expected) {
                found = Some(candidate);
                break;
            }
        }
        source = found?;
    }
    equivalent(&source, new).then_some(source)
}

// Conservative token discovery, not a second YAML parser. Every proposed edit
// must pass the real parser and match the entire ordered target document.
fn scalar_ranges(source: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        let bytes = text.as_bytes();
        let mut quote = None;
        let mut escaped = false;
        let mut start = text.len() - text.trim_start().len();
        if text[start..].starts_with("- ") {
            start += 2;
        }
        let mut end = text.len();
        let mut index = start;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(delimiter) = quote {
                if delimiter == b'"' && byte == b'\\' && !escaped {
                    escaped = true;
                    index += 1;
                    continue;
                }
                if byte == delimiter && !escaped {
                    if delimiter == b'\'' && bytes.get(index + 1) == Some(&b'\'') {
                        index += 2;
                        continue;
                    }
                    quote = None;
                }
                escaped = false;
            } else if byte == b'\'' || byte == b'"' {
                quote = Some(byte);
            } else if byte == b'#' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) {
                end = index;
                break;
            } else if byte == b':' && bytes.get(index + 1).is_none_or(u8::is_ascii_whitespace) {
                start = index + 1;
            }
            index += 1;
        }
        let token = &text[start.min(end)..end];
        let trimmed = token.trim();
        if !trimmed.is_empty() && !trimmed.starts_with(['|', '>', '&', '*', '!', '{', '[', '#']) {
            let leading = token.len() - token.trim_start().len();
            ranges.push(offset + start + leading..offset + start + leading + trimmed.len());
        }
        offset += line.len();
    }
    ranges
}

fn patch_root_fields(source: &str, old: &Value, new: &Value) -> Option<String> {
    let (old_fields, new_fields) = (old.as_object()?, new.as_object()?);
    let mut entries = Vec::new();
    let mut offset = 0;
    let mut document_end = source.len();
    for line in source.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        if text
            .strip_prefix("...")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            document_end = offset;
            break;
        }
        // Only discover column-one block keys. Parse the key alone so aliases,
        // anchors and multiline values do not hide an otherwise ordinary field.
        // Discovery remains conservative and every candidate is fully reparsed.
        if !line.starts_with(char::is_whitespace) && !line.starts_with(['#', '-', '%', '?', '{']) {
            for (index, character) in text.char_indices() {
                if character != ':'
                    || !text[index + 1..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
                {
                    continue;
                }
                let Ok(key) = serde_yaml::from_str::<serde_yaml::Value>(&text[..index]) else {
                    continue;
                };
                if let Ok(key) = scalar_key(key, "") {
                    entries.push((key, offset));
                    break;
                }
            }
        }
        offset += line.len();
    }
    if !entries.iter().map(|(key, _)| key).eq(old_fields.keys()) {
        return None;
    }
    let first_start = entries.first()?.1;
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut regions = std::collections::HashMap::with_capacity(entries.len());
    let mut previous_end = first_start;
    for (index, (key, start)) in entries.iter().enumerate() {
        let end = entries
            .get(index + 1)
            .map_or(document_end, |(_, start)| *start);
        let mut content_end = *start;
        let mut cursor = *start;
        for line in source[*start..end].split_inclusive('\n') {
            cursor += line.len();
            // Column-one comments introduce the next field or form a footer.
            // Indented comments may instead be literal block-scalar content.
            if !line.trim().is_empty() && !line.starts_with('#') {
                content_end = cursor;
            }
        }
        regions.insert(
            key.as_str(),
            (&source[previous_end..*start], &source[*start..content_end]),
        );
        previous_end = content_end;
    }
    let footer = &source[previous_end..];
    let mut regenerate = HashSet::new();
    loop {
        let mut result = source[..first_start].to_owned();
        if new_fields.is_empty() {
            result.push_str("{}");
            result.push_str(newline);
        }
        for (key, value) in new_fields {
            if let Some((leading, _)) = regions.get(key.as_str()) {
                result.push_str(leading);
            }
            if let Some((_, content)) = regions.get(key.as_str()).filter(|_| {
                old_fields
                    .get(key)
                    .is_some_and(|old| ordered_eq(old, value))
                    && !regenerate.contains(key.as_str())
            }) {
                result.push_str(content);
            } else {
                let mut field = Map::new();
                field.insert(key.clone(), value.clone());
                let mut rendered = emit_yaml(&Value::Object(field)).ok()?;
                if newline == "\r\n" {
                    rendered = rendered.replace('\n', newline);
                }
                result.push_str(&rendered);
            }
            // A formerly final field may have had no terminating newline.
            if !result.ends_with('\n') {
                result.push_str(newline);
            }
        }
        result.push_str(footer);
        if equivalent(&result, new) {
            return Some(result);
        }
        // Moving/removing an anchor definition can invalidate an unchanged
        // alias consumer. Normalize only source regions that cannot stand alone;
        // independent fields (and their comments) need not be sacrificed.
        let mut retry = false;
        for (key, value) in new_fields {
            let Some((_, content)) = regions.get(key.as_str()) else {
                continue;
            };
            if !old_fields
                .get(key)
                .is_some_and(|old| ordered_eq(old, value))
                || regenerate.contains(key.as_str())
            {
                continue;
            }
            let independent = parse_value(content).is_ok_and(|parsed| {
                parsed.as_object().is_some_and(|field| {
                    field.len() == 1
                        && field
                            .get(key)
                            .is_some_and(|actual| ordered_eq(actual, value))
                })
            });
            if !independent {
                regenerate.insert(key.as_str());
                retry = true;
            }
        }
        if !retry {
            return None;
        }
    }
}
