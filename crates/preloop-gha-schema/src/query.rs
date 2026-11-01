//! Queries over the definition graph of a parsed schema.
//!
//! Definitions reference each other through [`TypeRef`] values. Several of the
//! GitHub definitions are recursive (for example `steps-item` and
//! `parallel-step`), so every traversal in this module tracks visited
//! definition names and stops when it encounters one again.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::{Kind, Property, Schema, TypeRef};

/// Returns every definition reachable from `root` in depth-first discovery
/// order.
///
/// The first element is always `root` itself. Each definition appears at most
/// once, and references are followed through one-of members, sequence items,
/// mapping property types, and loose mapping key and value types. Builtin
/// types are skipped. Recursive definitions terminate because a definition is
/// only visited once. An unknown `root` yields an empty vector.
pub fn reachable_from<'a>(schema: &'a Schema, root: &str) -> Vec<&'a str> {
    let Some((root_name, _)) = schema.definitions.get_key_value(root) else {
        return Vec::new();
    };

    let mut visited = HashSet::new();
    let mut names = Vec::new();
    visit_reachable(schema, root_name.as_str(), &mut visited, &mut names);
    names
}

/// Returns the properties declared by `name` and by the definitions reachable
/// through its one-of members and sequence items, without following the types
/// of individual properties.
///
/// The returned map preserves discovery order. Since definitions can be
/// reached more than once, the first occurrence of a property name wins. An
/// unknown `name` yields an empty map.
pub fn flattened_properties<'a>(schema: &'a Schema, name: &str) -> IndexMap<&'a str, &'a Property> {
    let Some((root_name, _)) = schema.definitions.get_key_value(name) else {
        return IndexMap::new();
    };

    let mut visited = HashSet::new();
    let mut properties = IndexMap::new();
    collect_properties(schema, root_name.as_str(), &mut visited, &mut properties);
    properties
}

/// Appends `name` and everything reachable from it to `names`.
fn visit_reachable<'a>(
    schema: &'a Schema,
    name: &'a str,
    visited: &mut HashSet<&'a str>,
    names: &mut Vec<&'a str>,
) {
    if !visited.insert(name) {
        return;
    }
    names.push(name);

    let Some(definition) = schema.get(name) else {
        return;
    };

    match &definition.kind {
        Kind::Mapping {
            properties,
            loose_key,
            loose_value,
        } => {
            for property in properties.values() {
                visit_type(schema, &property.ty, visited, names);
            }
            if let Some(ty) = loose_key {
                visit_type(schema, ty, visited, names);
            }
            if let Some(ty) = loose_value {
                visit_type(schema, ty, visited, names);
            }
        }
        Kind::Sequence { item } => visit_type(schema, item, visited, names),
        Kind::OneOf { members } => {
            for member in members {
                visit_type(schema, member, visited, names);
            }
        }
        Kind::Null
        | Kind::Boolean
        | Kind::Number
        | Kind::String { .. }
        | Kind::AllowedValues { .. } => {}
    }
}

/// Follows a single type reference when it points at a definition.
fn visit_type<'a>(
    schema: &'a Schema,
    ty: &'a TypeRef,
    visited: &mut HashSet<&'a str>,
    names: &mut Vec<&'a str>,
) {
    if let TypeRef::Definition(name) = ty {
        visit_reachable(schema, name.as_str(), visited, names);
    }
}

/// Collects mapping properties reachable from `name` through one-of members and
/// sequence items.
fn collect_properties<'a>(
    schema: &'a Schema,
    name: &'a str,
    visited: &mut HashSet<&'a str>,
    properties: &mut IndexMap<&'a str, &'a Property>,
) {
    if !visited.insert(name) {
        return;
    }

    let Some(definition) = schema.get(name) else {
        return;
    };

    match &definition.kind {
        Kind::Mapping {
            properties: declared,
            ..
        } => {
            for (key, property) in declared {
                properties.entry(key.as_str()).or_insert(property);
            }
        }
        Kind::Sequence { item } => collect_type(schema, item, visited, properties),
        Kind::OneOf { members } => {
            for member in members {
                collect_type(schema, member, visited, properties);
            }
        }
        Kind::Null
        | Kind::Boolean
        | Kind::Number
        | Kind::String { .. }
        | Kind::AllowedValues { .. } => {}
    }
}

/// Follows a single type reference while collecting flattened properties.
fn collect_type<'a>(
    schema: &'a Schema,
    ty: &'a TypeRef,
    visited: &mut HashSet<&'a str>,
    properties: &mut IndexMap<&'a str, &'a Property>,
) {
    if let TypeRef::Definition(name) = ty {
        collect_properties(schema, name.as_str(), visited, properties);
    }
}
