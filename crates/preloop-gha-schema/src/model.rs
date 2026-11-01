//! The neutral, order-preserving model of GitHub's workflow and action
//! template schemas.
//!
//! A schema document is a map of named definitions. Each definition has an
//! optional description, an optional list of expression contexts in which it
//! may appear, and exactly one [`Kind`] describing its shape. Type references
//! ([`TypeRef`]) are either one of the builtin names or the name of another
//! definition in the same document.

use indexmap::IndexMap;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use thiserror::Error;

/// Errors produced while loading or validating a schema document.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// The document is not valid JSON.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// A definition body, property body or the document root is malformed.
    #[error("definition `{definition}` is invalid: {reason}")]
    InvalidDefinition {
        /// The definition whose body is malformed.
        definition: String,
        /// What is wrong with the body.
        reason: String,
    },

    /// A body carries a key the model does not describe.
    #[error("definition `{definition}` has unknown key `{key}`")]
    UnknownKey {
        /// The definition whose body carries the unknown key.
        definition: String,
        /// The offending key.
        key: String,
    },

    /// A type reference names neither a builtin nor an existing definition.
    #[error("definition `{definition}` references unknown type `{reference}`")]
    DanglingReference {
        /// The definition holding the bad reference.
        definition: String,
        /// The unresolved reference.
        reference: String,
    },

    /// A context entry is not of the form `name` or `name(min,max)`.
    #[error("definition `{definition}` has malformed context `{context}`")]
    MalformedContext {
        /// The definition declaring the context.
        definition: String,
        /// The offending context string.
        context: String,
    },

    /// A definition claims a name that is reserved for a builtin type.
    #[error("definition `{definition}` is named like a builtin type")]
    ReservedName {
        /// The definition using a reserved name.
        definition: String,
    },
}

/// A parsed GitHub Actions template schema document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Schema {
    /// The schema format version the document declares, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Every definition, in document order.
    pub definitions: IndexMap<String, Definition>,
}

impl Schema {
    /// Looks up a definition by name.
    pub fn get(&self, name: &str) -> Option<&Definition> {
        self.definitions.get(name)
    }

    /// Resolves a type reference to its definition.
    ///
    /// Returns `None` for builtin types, which have no definition body.
    pub fn resolve(&self, ty: &TypeRef) -> Option<&Definition> {
        ty.as_definition().and_then(|name| self.get(name))
    }
}

/// A single named definition inside a schema document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Definition {
    /// Human-readable description of the definition, when the schema has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Expression contexts in which the definition may appear.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contexts: Vec<ContextSpec>,

    /// The shape the definition describes.
    #[serde(flatten)]
    pub kind: Kind,
}

/// The shape described by a single definition.
///
/// Exactly one variant applies to a definition; the JSON form is tagged with a
/// kebab-case `kind` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Kind {
    /// The YAML `null` value.
    Null,

    /// A boolean value.
    Boolean,

    /// A numeric value.
    Number,

    /// A string value, optionally pinned to a single constant.
    String {
        /// The only value this string may hold, when the schema pins one.
        #[serde(skip_serializing_if = "Option::is_none")]
        constant: Option<String>,

        /// Whether the string must not be empty.
        #[serde(rename = "requireNonEmpty", skip_serializing_if = "is_false")]
        require_non_empty: bool,
    },

    /// One value out of a fixed list.
    AllowedValues {
        /// Every accepted value, in document order.
        values: Vec<String>,
    },

    /// An ordered list whose entries share a single type.
    Sequence {
        /// The type every entry must satisfy.
        item: TypeRef,
    },

    /// A string-keyed map.
    Mapping {
        /// Declared properties, in document order.
        properties: IndexMap<String, Property>,

        /// The type every undeclared key must satisfy.
        #[serde(rename = "looseKey", skip_serializing_if = "Option::is_none")]
        loose_key: Option<TypeRef>,

        /// The type every undeclared value must satisfy.
        #[serde(rename = "looseValue", skip_serializing_if = "Option::is_none")]
        loose_value: Option<TypeRef>,
    },

    /// A value matching one of several shapes.
    OneOf {
        /// The accepted alternatives, in document order.
        members: Vec<TypeRef>,
    },
}

/// A declared property of a mapping definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Property {
    /// The type the property value must satisfy.
    #[serde(rename = "type")]
    pub ty: TypeRef,

    /// Whether the property must be present.
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,

    /// Human-readable description of the property, when the schema has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A reference to either a builtin type or another definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum TypeRef {
    /// A type built into the schema format.
    Builtin(BuiltinType),

    /// The name of another definition in the same document.
    Definition(String),
}

impl TypeRef {
    /// The referenced definition name, or `None` for a builtin type.
    pub fn as_definition(&self) -> Option<&str> {
        match self {
            TypeRef::Builtin(_) => None,
            TypeRef::Definition(name) => Some(name),
        }
    }
}

/// A type built into the schema format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinType {
    /// The YAML `null` value.
    Null,

    /// A boolean value.
    Boolean,

    /// A numeric value.
    Number,

    /// A string value.
    String,

    /// An ordered list.
    Sequence,

    /// A string-keyed map.
    Mapping,

    /// Any value at all, used by loose map entries and values.
    Any,
}

impl BuiltinType {
    /// The name this builtin uses inside a schema document.
    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinType::Null => "null",
            BuiltinType::Boolean => "boolean",
            BuiltinType::Number => "number",
            BuiltinType::String => "string",
            BuiltinType::Sequence => "sequence",
            BuiltinType::Mapping => "mapping",
            BuiltinType::Any => "any",
        }
    }

    /// Parses a builtin type name, returning `None` when `name` is not a
    /// builtin and therefore refers to a definition.
    pub fn from_name(name: &str) -> Option<BuiltinType> {
        match name {
            "null" => Some(BuiltinType::Null),
            "boolean" => Some(BuiltinType::Boolean),
            "number" => Some(BuiltinType::Number),
            "string" => Some(BuiltinType::String),
            "sequence" => Some(BuiltinType::Sequence),
            "mapping" => Some(BuiltinType::Mapping),
            "any" => Some(BuiltinType::Any),
            _ => None,
        }
    }
}

/// An expression context a definition may appear in.
///
/// A context without arity is a plain context such as `github`. A context with
/// arity is a function call such as `hashFiles(1,255)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSpec {
    /// The context name, for example `github` or `hashFiles`.
    pub name: String,

    /// The accepted argument count, present only for function contexts.
    pub arity: Option<Arity>,
}

impl Serialize for ContextSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let len = match &self.arity {
            Some(arity) if arity.max.is_some() => 3,
            Some(_) => 2,
            None => 1,
        };
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("name", &self.name)?;
        if let Some(arity) = &self.arity {
            map.serialize_entry("min", &arity.min)?;
            if let Some(max) = arity.max {
                map.serialize_entry("max", &max)?;
            }
        }
        map.end()
    }
}

/// The accepted argument count of a function context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Arity {
    /// The smallest accepted argument count.
    pub min: usize,

    /// The largest accepted argument count; `None` means unbounded (`MAX`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

/// Returns `true` for `false`, used to omit unset boolean flags when
/// serializing.
fn is_false(value: &bool) -> bool {
    !*value
}
