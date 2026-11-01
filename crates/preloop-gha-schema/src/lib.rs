//! A typed, order-preserving model of GitHub's workflow and action template
//! schemas.
//!
//! The crate embeds the two vendored schema files and parses them on first
//! use, exposing the workflow schema through [`workflow`] and the action schema
//! through [`action`]. [`WORKFLOW_ROOT`] and [`ACTION_ROOT`] name the strict
//! root definition of each schema.
//!
//! Loading is strict: only the keys the model describes are accepted, and every
//! type reference must resolve to a builtin or to another definition.

#![forbid(unsafe_code)]

mod load;
mod model;
pub mod query;

pub use model::{
    Arity, BuiltinType, ContextSpec, Definition, Kind, Property, Schema, SchemaError, TypeRef,
};

use std::sync::OnceLock;

/// The strict workflow root definition inside the workflow schema.
pub const WORKFLOW_ROOT: &str = "workflow-root-strict";

/// The strict action root definition inside the action schema.
pub const ACTION_ROOT: &str = "action-root-strict";

/// The embedded workflow template schema.
const WORKFLOW_SCHEMA: &str = include_str!("../schema/workflow-v1.0.json");

/// The embedded action template schema.
const ACTION_SCHEMA: &str = include_str!("../schema/action-v1.0.json");

/// The workflow schema, parsed once on first access.
static WORKFLOW: OnceLock<Schema> = OnceLock::new();

/// The action schema, parsed once on first access.
static ACTION: OnceLock<Schema> = OnceLock::new();

/// Returns the parsed workflow template schema.
pub fn workflow() -> &'static Schema {
    WORKFLOW.get_or_init(|| {
        Schema::parse(WORKFLOW_SCHEMA)
            .expect("embedded workflow schema must parse; the crate tests guard this file")
    })
}

/// Returns the parsed action template schema.
pub fn action() -> &'static Schema {
    ACTION.get_or_init(|| {
        Schema::parse(ACTION_SCHEMA)
            .expect("embedded action schema must parse; the crate tests guard this file")
    })
}
