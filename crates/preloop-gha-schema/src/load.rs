//! Strict loading of schema JSON documents into [`Schema`].
//!
//! The loader walks `serde_json::Value` by hand instead of deriving
//! `Deserialize`, so that every key, kind and type reference is checked against
//! the model and failures name the offending definition.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::model::{
    Arity, BuiltinType, ContextSpec, Definition, Kind, Property, Schema, SchemaError, TypeRef,
};

/// Label used in errors for keys and values outside any definition body.
const ROOT: &str = "<root>";

/// Every key a definition body may carry.
const BODY_KEYS: &[&str] = &[
    "description",
    "context",
    "mapping",
    "sequence",
    "one-of",
    "string",
    "boolean",
    "number",
    "null",
    "allowed-values",
];

/// Every key that selects a definition kind.
const KIND_KEYS: &[&str] = &[
    "mapping",
    "sequence",
    "one-of",
    "string",
    "boolean",
    "number",
    "null",
    "allowed-values",
];

impl Schema {
    /// Parses a template schema document.
    ///
    /// Parsing is strict: unknown keys, bodies without exactly one kind,
    /// malformed context strings, definitions named like builtin types and
    /// references to missing definitions all fail with a [`SchemaError`] that
    /// names the offending definition.
    pub fn parse(json: &str) -> Result<Schema, SchemaError> {
        let value: Value = serde_json::from_str(json)?;
        let root = expect_object(&value, ROOT, "schema document")?;
        check_keys(root, ROOT, &["version", "definitions"])?;

        let version = match root.get("version") {
            Some(value) => Some(expect_string(value, ROOT, "`version`")?.to_owned()),
            None => None,
        };

        let raw_definitions = root
            .get("definitions")
            .ok_or_else(|| invalid(ROOT, "missing `definitions`"))?;
        let raw_definitions = expect_object(raw_definitions, ROOT, "`definitions`")?;

        for name in raw_definitions.keys() {
            if BuiltinType::from_name(name).is_some() {
                return Err(SchemaError::ReservedName {
                    definition: name.clone(),
                });
            }
        }

        let mut definitions = IndexMap::with_capacity(raw_definitions.len());
        for (name, body) in raw_definitions {
            definitions.insert(name.clone(), parse_definition(name, body)?);
        }

        validate_references(&definitions)?;

        Ok(Schema {
            version,
            definitions,
        })
    }
}

/// Parses one definition body, rejecting unknown keys and ambiguous kinds.
fn parse_definition(name: &str, body: &Value) -> Result<Definition, SchemaError> {
    let object = expect_object(body, name, "definition body")?;
    check_keys(object, name, BODY_KEYS)?;

    let description = match object.get("description") {
        Some(value) => Some(expect_string(value, name, "`description`")?.to_owned()),
        None => None,
    };

    let contexts = match object.get("context") {
        Some(value) => parse_contexts(value, name)?,
        None => Vec::new(),
    };

    let present: Vec<&str> = KIND_KEYS
        .iter()
        .copied()
        .filter(|key| object.contains_key(*key))
        .collect();

    if present.len() != 1 {
        let found = if present.is_empty() {
            "none".to_owned()
        } else {
            present.join(", ")
        };
        return Err(invalid(
            name,
            format!("expected exactly one kind key, found {found}"),
        ));
    }

    let kind_key = present[0];
    let kind_value = object
        .get(kind_key)
        .expect("kind key presence was checked above");
    let kind = parse_kind(kind_key, kind_value, name)?;

    Ok(Definition {
        description,
        contexts,
        kind,
    })
}

/// Parses the body of the kind selected by `key`.
fn parse_kind(key: &str, value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    match key {
        "null" => {
            expect_empty_object(value, definition, "`null`")?;
            Ok(Kind::Null)
        }
        "boolean" => {
            expect_empty_object(value, definition, "`boolean`")?;
            Ok(Kind::Boolean)
        }
        "number" => {
            expect_empty_object(value, definition, "`number`")?;
            Ok(Kind::Number)
        }
        "string" => parse_string_kind(value, definition),
        "allowed-values" => parse_allowed_values(value, definition),
        "sequence" => parse_sequence(value, definition),
        "mapping" => parse_mapping(value, definition),
        "one-of" => parse_one_of(value, definition),
        other => Err(invalid(
            definition,
            format!("unsupported kind key `{other}`"),
        )),
    }
}

/// Parses a `string` body, which may pin a constant or forbid empty values.
fn parse_string_kind(value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    let object = expect_object(value, definition, "`string`")?;
    check_keys(object, definition, &["constant", "require-non-empty"])?;

    let constant = match object.get("constant") {
        Some(value) => Some(expect_string(value, definition, "`constant`")?.to_owned()),
        None => None,
    };

    let require_non_empty = match object.get("require-non-empty") {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| invalid(definition, "`require-non-empty` must be a boolean"))?,
        None => false,
    };

    Ok(Kind::String {
        constant,
        require_non_empty,
    })
}

/// Parses an `allowed-values` body into the accepted strings.
fn parse_allowed_values(value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    let array = expect_array(value, definition, "`allowed-values`")?;
    let mut values = Vec::with_capacity(array.len());
    for entry in array {
        values.push(expect_string(entry, definition, "`allowed-values` entry")?.to_owned());
    }
    Ok(Kind::AllowedValues { values })
}

/// Parses a `sequence` body.
fn parse_sequence(value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    let object = expect_object(value, definition, "`sequence`")?;
    check_keys(object, definition, &["item-type"])?;

    let item = object
        .get("item-type")
        .ok_or_else(|| invalid(definition, "missing `item-type`"))?;
    let item = type_ref_from_value(item, definition, "`item-type`")?;

    Ok(Kind::Sequence { item })
}

/// Parses a `mapping` body, including its declared and loose entries.
fn parse_mapping(value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    let object = expect_object(value, definition, "`mapping`")?;
    check_keys(
        object,
        definition,
        &["properties", "loose-key-type", "loose-value-type"],
    )?;

    let mut properties = IndexMap::new();
    if let Some(raw) = object.get("properties") {
        let raw = expect_object(raw, definition, "`properties`")?;
        for (name, value) in raw {
            properties.insert(name.clone(), parse_property(definition, name, value)?);
        }
    }

    let loose_key = object
        .get("loose-key-type")
        .map(|value| type_ref_from_value(value, definition, "`loose-key-type`"))
        .transpose()?;
    let loose_value = object
        .get("loose-value-type")
        .map(|value| type_ref_from_value(value, definition, "`loose-value-type`"))
        .transpose()?;

    Ok(Kind::Mapping {
        properties,
        loose_key,
        loose_value,
    })
}

/// Parses a `one-of` body into its accepted alternatives.
fn parse_one_of(value: &Value, definition: &str) -> Result<Kind, SchemaError> {
    let array = expect_array(value, definition, "`one-of`")?;
    let mut members = Vec::with_capacity(array.len());
    for entry in array {
        members.push(type_ref_from_value(entry, definition, "`one-of` member")?);
    }
    Ok(Kind::OneOf { members })
}

/// Parses one entry of a mapping's `properties` object.
fn parse_property(definition: &str, name: &str, value: &Value) -> Result<Property, SchemaError> {
    match value {
        Value::String(ty) => Ok(Property {
            ty: type_ref_from_name(ty),
            required: false,
            description: None,
        }),
        Value::Object(object) => {
            check_keys(object, definition, &["type", "required", "description"])?;

            let ty = object.get("type").ok_or_else(|| {
                invalid(definition, format!("property `{name}` is missing `type`"))
            })?;
            let ty = type_ref_from_value(ty, definition, "property `type`")?;

            let required = match object.get("required") {
                Some(value) => value.as_bool().ok_or_else(|| {
                    invalid(
                        definition,
                        format!("property `{name}` `required` must be a boolean"),
                    )
                })?,
                None => false,
            };

            let description = match object.get("description") {
                Some(value) => {
                    let what = format!("property `{name}` description");
                    Some(expect_string(value, definition, &what)?.to_owned())
                }
                None => None,
            };

            Ok(Property {
                ty,
                required,
                description,
            })
        }
        _ => Err(invalid(
            definition,
            format!("property `{name}` must be a type name string or an object"),
        )),
    }
}

/// Parses a `context` array into context specifications.
fn parse_contexts(value: &Value, definition: &str) -> Result<Vec<ContextSpec>, SchemaError> {
    let array = expect_array(value, definition, "`context`")?;
    let mut contexts = Vec::with_capacity(array.len());
    for entry in array {
        let text = expect_string(entry, definition, "`context` entry")?;
        let context = parse_context(text).ok_or_else(|| SchemaError::MalformedContext {
            definition: definition.to_owned(),
            context: text.to_owned(),
        })?;
        contexts.push(context);
    }
    Ok(contexts)
}

/// Parses a single context string, accepting `name` and `name(min,max)` where
/// `max` may be `MAX` for an unbounded function.
fn parse_context(text: &str) -> Option<ContextSpec> {
    let Some(open) = text.find('(') else {
        if !is_identifier(text) {
            return None;
        }
        return Some(ContextSpec {
            name: text.to_owned(),
            arity: None,
        });
    };

    let name = &text[..open];
    if !is_identifier(name) {
        return None;
    }

    let arguments = text.get(open + 1..)?.strip_suffix(')')?;
    let (min, max) = arguments.split_once(',')?;
    let min = parse_arity_bound(min)?;
    let max = if max == "MAX" {
        None
    } else {
        Some(parse_arity_bound(max)?)
    };

    Some(ContextSpec {
        name: name.to_owned(),
        arity: Some(Arity { min, max }),
    })
}

/// Returns `true` when `text` is an ASCII identifier.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Parses one non-negative arity bound written in decimal digits.
fn parse_arity_bound(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok()
}

/// Converts a type reference string into a builtin or definition reference.
fn type_ref_from_name(name: &str) -> TypeRef {
    match BuiltinType::from_name(name) {
        Some(builtin) => TypeRef::Builtin(builtin),
        None => TypeRef::Definition(name.to_owned()),
    }
}

/// Parses a JSON string into a type reference.
fn type_ref_from_value(
    value: &Value,
    definition: &str,
    what: &str,
) -> Result<TypeRef, SchemaError> {
    let name = expect_string(value, definition, what)?;
    Ok(type_ref_from_name(name))
}

/// Fails when `definitions` holds a reference to a name that does not exist.
fn validate_references(definitions: &IndexMap<String, Definition>) -> Result<(), SchemaError> {
    for (name, definition) in definitions {
        let mut dangling: Option<String> = None;
        visit_type_refs(&definition.kind, |ty| {
            if dangling.is_some() {
                return;
            }
            if let Some(target) = ty.as_definition() {
                if !definitions.contains_key(target) {
                    dangling = Some(target.to_owned());
                }
            }
        });
        if let Some(reference) = dangling {
            return Err(SchemaError::DanglingReference {
                definition: name.clone(),
                reference,
            });
        }
    }
    Ok(())
}

/// Calls `visit` for every type reference directly held by `kind`.
fn visit_type_refs<'a>(kind: &'a Kind, mut visit: impl FnMut(&'a TypeRef)) {
    match kind {
        Kind::Null
        | Kind::Boolean
        | Kind::Number
        | Kind::String { .. }
        | Kind::AllowedValues { .. } => {}
        Kind::Sequence { item } => visit(item),
        Kind::OneOf { members } => {
            for member in members {
                visit(member);
            }
        }
        Kind::Mapping {
            properties,
            loose_key,
            loose_value,
        } => {
            for property in properties.values() {
                visit(&property.ty);
            }
            if let Some(ty) = loose_key {
                visit(ty);
            }
            if let Some(ty) = loose_value {
                visit(ty);
            }
        }
    }
}

/// Builds an "invalid definition" error.
fn invalid(definition: &str, reason: impl Into<String>) -> SchemaError {
    SchemaError::InvalidDefinition {
        definition: definition.to_owned(),
        reason: reason.into(),
    }
}

/// Requires `value` to be a JSON object.
fn expect_object<'a>(
    value: &'a Value,
    definition: &str,
    what: &str,
) -> Result<&'a Map<String, Value>, SchemaError> {
    value
        .as_object()
        .ok_or_else(|| invalid(definition, format!("{what} must be a JSON object")))
}

/// Requires `value` to be a JSON array.
fn expect_array<'a>(
    value: &'a Value,
    definition: &str,
    what: &str,
) -> Result<&'a Vec<Value>, SchemaError> {
    value
        .as_array()
        .ok_or_else(|| invalid(definition, format!("{what} must be a JSON array")))
}

/// Requires `value` to be a JSON string.
fn expect_string<'a>(
    value: &'a Value,
    definition: &str,
    what: &str,
) -> Result<&'a str, SchemaError> {
    value
        .as_str()
        .ok_or_else(|| invalid(definition, format!("{what} must be a string")))
}

/// Rejects any key of `object` that is not listed in `allowed`.
fn check_keys(
    object: &Map<String, Value>,
    definition: &str,
    allowed: &[&str],
) -> Result<(), SchemaError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(SchemaError::UnknownKey {
                definition: definition.to_owned(),
                key: key.clone(),
            });
        }
    }
    Ok(())
}

/// Requires `value` to be an empty JSON object.
fn expect_empty_object(value: &Value, definition: &str, what: &str) -> Result<(), SchemaError> {
    let object = expect_object(value, definition, what)?;
    check_keys(object, definition, &[])
}
