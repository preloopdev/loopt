//! Integration tests for the schema model and the graph queries over it.
//!
//! Every test exercises the public API only, so this file also acts as a
//! contract check for downstream consumers such as the TypeScript generator.

use preloop_gha_schema::query::{flattened_properties, reachable_from};
use preloop_gha_schema::{action, workflow, Kind, Schema, TypeRef, ACTION_ROOT, WORKFLOW_ROOT};
use serde_json::json;

/// Walks every definition and asserts that each named reference resolves.
fn assert_all_references_resolve(schema: &Schema) {
    for (name, definition) in &schema.definitions {
        match &definition.kind {
            Kind::Mapping {
                properties,
                loose_key,
                loose_value,
            } => {
                for (key, property) in properties {
                    assert_resolves(schema, &property.ty, name, key);
                }
                if let Some(ty) = loose_key {
                    assert_resolves(schema, ty, name, "<loose-key-type>");
                }
                if let Some(ty) = loose_value {
                    assert_resolves(schema, ty, name, "<loose-value-type>");
                }
            }
            Kind::Sequence { item } => assert_resolves(schema, item, name, "<item-type>"),
            Kind::OneOf { members } => {
                for (index, member) in members.iter().enumerate() {
                    assert_resolves(schema, member, name, &format!("<member {index}>"));
                }
            }
            _ => {}
        }
    }
}

/// Asserts that a reference either names an existing definition or is a
/// builtin, and that `Schema::resolve` agrees.
fn assert_resolves(schema: &Schema, ty: &TypeRef, owner: &str, site: &str) {
    match ty {
        TypeRef::Definition(name) => {
            assert!(
                schema.resolve(ty).is_some(),
                "definition `{owner}` has a dangling reference to `{name}` at {site}"
            );
            assert!(
                schema.get(name).is_some(),
                "definition `{owner}` references missing definition `{name}` at {site}"
            );
        }
        TypeRef::Builtin(_) => {
            assert!(
                schema.resolve(ty).is_none(),
                "builtin reference in `{owner}` at {site} unexpectedly resolved to a definition"
            );
        }
    }
}

#[test]
fn schema_definition_counts_are_stable() {
    // These counts pin the vendored schemas. Bump them when the schema files
    // are updated, and review the reference-resolution test alongside them.
    assert_eq!(workflow().definitions.len(), 304);
    assert_eq!(action().definitions.len(), 37);
}

#[test]
fn every_named_reference_resolves() {
    assert_all_references_resolve(workflow());
    assert_all_references_resolve(action());
}

#[test]
fn strict_parsing_rejects_unknown_definition_keys() {
    let document = r#"{
        "definitions": {
            "example": {
                "description": "A definition with an unknown key.",
                "mapping": { "properties": {} },
                "unexpected-key": true
            }
        }
    }"#;

    let error = Schema::parse(document).expect_err("unknown definition key must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("example"),
        "error should name the offending definition, got: {message}"
    );
}

#[test]
fn strict_parsing_rejects_dangling_references() {
    let document = r#"{
        "definitions": {
            "example": {
                "mapping": {
                    "properties": {
                        "value": "missing-definition"
                    }
                }
            }
        }
    }"#;

    let error = Schema::parse(document).expect_err("dangling reference must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("example"),
        "error should name the offending definition, got: {message}"
    );
}

#[test]
fn strict_parsing_rejects_multiple_kind_keys() {
    let document = r#"{
        "definitions": {
            "example": {
                "mapping": { "properties": {} },
                "string": {}
            }
        }
    }"#;

    let error = Schema::parse(document).expect_err("two kind keys must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("example"),
        "error should name the offending definition, got: {message}"
    );
}

#[test]
fn job_if_exposes_plain_and_function_contexts() {
    let job_if = workflow().get("job-if").expect("job-if definition");

    let plain: Vec<&str> = job_if
        .contexts
        .iter()
        .filter(|candidate| candidate.arity.is_none())
        .map(|candidate| candidate.name.as_str())
        .collect();
    assert_eq!(plain, vec!["github", "inputs", "vars", "needs"]);

    let always = job_if
        .contexts
        .iter()
        .find(|candidate| candidate.name == "always")
        .expect("always context")
        .arity
        .as_ref()
        .expect("always is a function context");
    assert_eq!(always.min, 0);
    assert_eq!(always.max, Some(0));

    let failure = job_if
        .contexts
        .iter()
        .find(|candidate| candidate.name == "failure")
        .expect("failure context")
        .arity
        .as_ref()
        .expect("failure is a function context");
    assert_eq!(failure.min, 0);
    assert_eq!(failure.max, None);
}

#[test]
fn step_if_exposes_hash_files_arity() {
    let step_if = workflow().get("step-if").expect("step-if definition");
    let hash_files = step_if
        .contexts
        .iter()
        .find(|candidate| candidate.name == "hashFiles")
        .expect("hashFiles context")
        .arity
        .as_ref()
        .expect("hashFiles is a function context");
    assert_eq!(hash_files.min, 1);
    assert_eq!(hash_files.max, Some(255));
}

#[test]
fn workflow_root_is_reachable_without_duplicates() {
    let names = reachable_from(workflow(), WORKFLOW_ROOT);

    assert_eq!(names.first().copied(), Some(WORKFLOW_ROOT));
    for expected in ["steps-item", "run-step", "permissions-mapping"] {
        assert!(
            names.contains(&expected),
            "`{expected}` should be reachable from `{WORKFLOW_ROOT}`"
        );
    }

    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "reachable definitions must not contain duplicates"
    );

    assert!(
        !reachable_from(action(), ACTION_ROOT).is_empty(),
        "the action root must reach at least itself"
    );
}

#[test]
fn flattened_properties_cover_event_and_permission_mappings() {
    let on = flattened_properties(workflow(), "on-strict");
    assert_eq!(on.len(), 37);
    for expected in ["push", "pull_request", "workflow_dispatch"] {
        assert!(
            on.contains_key(expected),
            "`on-strict` should contain `{expected}`"
        );
    }

    let permissions = flattened_properties(workflow(), "permissions-mapping");
    assert_eq!(permissions.len(), 20);

    let job = flattened_properties(workflow(), "job");
    for expected in ["runs-on", "steps", "uses", "secrets", "snapshot"] {
        assert!(
            job.contains_key(expected),
            "`job` should contain `{expected}`"
        );
    }
}

#[test]
fn serialization_matches_the_contract_shape() {
    let job_if = workflow().get("job-if").expect("job-if definition");
    assert_eq!(
        serde_json::to_value(job_if).expect("job-if serializes"),
        json!({
            "description": "You can use the `if` conditional to prevent a job from running unless a condition is met. You can use any supported context and expression to create a conditional.",
            "contexts": [
                { "name": "github" },
                { "name": "inputs" },
                { "name": "vars" },
                { "name": "needs" },
                { "name": "always", "min": 0, "max": 0 },
                { "name": "failure", "min": 0 },
                { "name": "cancelled", "min": 0, "max": 0 },
                { "name": "success", "min": 0 }
            ],
            "kind": "string"
        })
    );

    let permission_level_any = workflow()
        .get("permission-level-any")
        .expect("permission-level-any definition");
    assert_eq!(
        serde_json::to_value(permission_level_any).expect("permission-level-any serializes"),
        json!({
            "description": "The permission level for the `GITHUB_TOKEN`.",
            "kind": "one-of",
            "members": [
                "permission-level-read",
                "permission-level-write",
                "permission-level-no-access"
            ]
        })
    );

    let container_mapping = workflow()
        .get("container-mapping")
        .expect("container-mapping definition");
    assert_eq!(
        serde_json::to_value(container_mapping).expect("container-mapping serializes"),
        json!({
            "kind": "mapping",
            "properties": {
                "image": {
                    "type": "non-empty-string",
                    "description": "Use `jobs.<job_id>.container.image` to define the Docker image to use as the container to run the action. The value can be the Docker Hub image or a registry name."
                },
                "options": {
                    "type": "string",
                    "description": "Use `jobs.<job_id>.container.options` to configure additional Docker container resource options."
                },
                "env": {
                    "type": "container-env"
                },
                "ports": {
                    "type": "sequence-of-non-empty-string",
                    "description": "Use `jobs.<job_id>.container.ports` to set an array of ports to expose on the container."
                },
                "volumes": {
                    "type": "sequence-of-non-empty-string",
                    "description": "Use `jobs.<job_id>.container.volumes` to set an array of volumes for the container to use. You can use volumes to share data between services or other steps in a job. You can specify named Docker volumes, anonymous Docker volumes, or bind mounts on the host."
                },
                "credentials": {
                    "type": "container-registry-credentials"
                }
            }
        })
    );

    let sequence = workflow()
        .get("sequence-of-non-empty-string")
        .expect("sequence-of-non-empty-string definition");
    assert_eq!(
        serde_json::to_value(sequence).expect("sequence serializes"),
        json!({
            "kind": "sequence",
            "item": "non-empty-string"
        })
    );

    let string_constant = workflow()
        .get("input-type-string")
        .expect("input-type-string definition");
    assert_eq!(
        serde_json::to_value(string_constant).expect("string constant serializes"),
        json!({
            "kind": "string",
            "constant": "string"
        })
    );
}

#[test]
fn run_step_requires_run_with_documentation() {
    let run_step = workflow().get("run-step").expect("run-step definition");
    let Kind::Mapping { properties, .. } = &run_step.kind else {
        panic!("run-step should be a mapping");
    };
    let run = properties.get("run").expect("run property");
    assert!(run.required, "run should be required");
    assert!(
        run.description
            .as_deref()
            .is_some_and(|text| !text.is_empty()),
        "run should carry a description"
    );
}

#[test]
fn unknown_roots_yield_empty_results() {
    assert!(reachable_from(workflow(), "not-a-definition").is_empty());
    assert!(flattened_properties(workflow(), "not-a-definition").is_empty());
}
