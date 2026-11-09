//! Shape validation for `workflow_dispatch` and `workflow_call` inputs,
//! outputs, and secrets.
//!
//! Reference:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#onworkflow_dispatchinputs>
//! and
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#onworkflow_callinputs>.

use serde_json::{Map, Value};

use crate::Diagnostic;

use super::{catalog, error, escape};

pub(super) fn check_dispatch(config: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(config) = config.as_object() else {
        return;
    };
    let Some(inputs) = config.get("inputs").and_then(Value::as_object) else {
        return;
    };
    for (id, input) in inputs {
        let input_path = format!("{path}/inputs/{}", escape(id));
        check_input_id(id, &input_path, findings);
        let Some(input) = input.as_object() else {
            findings.push(error(
                "preloop/invalid-dispatch-input",
                &input_path,
                format!("input `{id}` must be a mapping of input settings"),
                "write the input as a mapping with `description`, `required`, `default`, and `type`",
            ));
            continue;
        };
        check_dispatch_input(id, input, &input_path, findings);
    }
}

fn check_dispatch_input(
    id: &str,
    input: &Map<String, Value>,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    let input_type = match input.get("type") {
        None => None,
        Some(Value::String(text)) if text.contains("${{") => return,
        Some(Value::String(text)) => {
            if !catalog::DISPATCH_INPUT_TYPES.contains(&text.as_str()) {
                findings.push(error(
                    "preloop/invalid-dispatch-input",
                    &format!("{path}/type"),
                    format!(
                        "input `{id}` has unknown type `{text}`; expected one of {}",
                        list(catalog::DISPATCH_INPUT_TYPES)
                    ),
                    "use `string`, `number`, `boolean`, `choice`, or `environment`",
                ));
                return;
            }
            Some(text.as_str())
        }
        Some(_) => {
            findings.push(error(
                "preloop/invalid-dispatch-input",
                &format!("{path}/type"),
                format!("`type` of input `{id}` must be a string"),
                "use `string`, `number`, `boolean`, `choice`, or `environment`",
            ));
            return;
        }
    };

    match (input_type, input.get("options")) {
        (Some("choice"), None) => findings.push(error(
            "preloop/invalid-dispatch-input",
            &format!("{path}/options"),
            format!("input `{id}` is a `choice` but has no `options`"),
            "add a non-empty `options` list",
        )),
        (Some("choice"), Some(options)) => check_choice_options(
            id,
            options,
            input.get("default"),
            &format!("{path}/options"),
            &format!("{path}/default"),
            findings,
        ),
        (_, Some(_)) => findings.push(error(
            "preloop/invalid-dispatch-input",
            &format!("{path}/options"),
            format!("`options` is only valid when the type of input `{id}` is `choice`"),
            "remove `options`, or change `type` to `choice`",
        )),
        (_, None) => {}
    }

    match input_type {
        Some("boolean") => check_boolean_default(id, input.get("default"), path, findings),
        Some("number") => check_number_default(id, input.get("default"), path, findings),
        _ => {}
    }
}

fn check_choice_options(
    id: &str,
    options: &Value,
    default: Option<&Value>,
    options_path: &str,
    default_path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(options) = options.as_array() else {
        findings.push(error(
            "preloop/invalid-dispatch-input",
            options_path,
            format!("`options` of input `{id}` must be a list of strings"),
            "write each allowed value as a list item",
        ));
        return;
    };
    if options.is_empty() {
        findings.push(error(
            "preloop/invalid-dispatch-input",
            options_path,
            format!("`options` of input `{id}` must contain at least one value"),
            "add the values the user can choose from",
        ));
        return;
    }

    let mut allowed = Vec::with_capacity(options.len());
    for option in options {
        match option {
            Value::String(text) if text.contains("${{") => return,
            Value::String(text) => allowed.push(text.clone()),
            Value::Number(number) => allowed.push(number.to_string()),
            Value::Bool(value) => allowed.push(value.to_string()),
            _ => {
                findings.push(error(
                    "preloop/invalid-dispatch-input",
                    options_path,
                    format!("`options` of input `{id}` must contain only strings"),
                    "write each allowed value as a string",
                ));
                return;
            }
        }
    }

    let Some(default) = default else {
        return;
    };
    let default = match default {
        Value::Null => return,
        Value::String(text) if text.contains("${{") => return,
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => {
            findings.push(error(
                "preloop/invalid-dispatch-input",
                default_path,
                format!("default of input `{id}` must be one of its `options`"),
                "use one of the values listed in `options`",
            ));
            return;
        }
    };
    if !allowed.contains(&default) {
        findings.push(error(
            "preloop/invalid-dispatch-input",
            default_path,
            format!(
                "default `{default}` of input `{id}` is not one of its options ({})",
                list(&allowed)
            ),
            "use one of the values listed in `options`",
        ));
    }
}

fn check_boolean_default(
    id: &str,
    default: Option<&Value>,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    match default {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) if text.contains("${{") => {}
        Some(Value::Bool(_)) => {}
        Some(Value::String(text))
            if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") => {}
        Some(_) => findings.push(error(
            "preloop/invalid-dispatch-input",
            &format!("{path}/default"),
            format!("default of boolean input `{id}` must be `true` or `false`"),
            "write the default as an unquoted YAML boolean",
        )),
    }
}

fn check_number_default(
    id: &str,
    default: Option<&Value>,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    match default {
        None | Some(Value::Null) => {}
        Some(Value::Number(_)) => {}
        Some(Value::String(text)) if text.contains("${{") => {}
        Some(Value::String(text)) if text.parse::<f64>().is_ok() => {}
        Some(_) => findings.push(error(
            "preloop/invalid-dispatch-input",
            &format!("{path}/default"),
            format!("default of number input `{id}` must be a number"),
            "write the default as a numeric YAML value",
        )),
    }
}

pub(super) fn check_call(config: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(config) = config.as_object() else {
        return;
    };
    if let Some(inputs) = config.get("inputs").and_then(Value::as_object) {
        for (id, input) in inputs {
            let input_path = format!("{path}/inputs/{}", escape(id));
            check_input_id(id, &input_path, findings);
            let Some(input) = input.as_object() else {
                findings.push(error(
                    "preloop/invalid-dispatch-input",
                    &input_path,
                    format!("input `{id}` must be a mapping of input settings"),
                    "write the input as a mapping with `description`, `required`, `default`, and `type`",
                ));
                continue;
            };
            check_call_input(id, input, &input_path, findings);
        }
    }
    if let Some(outputs) = config.get("outputs").and_then(Value::as_object) {
        for (id, output) in outputs {
            check_output(
                id,
                output,
                &format!("{path}/outputs/{}", escape(id)),
                findings,
            );
        }
    }
    if let Some(secrets) = config.get("secrets").and_then(Value::as_object) {
        for (id, secret) in secrets {
            check_secret(
                id,
                secret,
                &format!("{path}/secrets/{}", escape(id)),
                findings,
            );
        }
    }
}

fn check_call_input(
    id: &str,
    input: &Map<String, Value>,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    let input_type = match input.get("type") {
        None => {
            findings.push(error(
                "preloop/invalid-dispatch-input",
                &format!("{path}/type"),
                format!("input `{id}` of `workflow_call` is missing the required `type`"),
                "add `type: boolean`, `type: number`, or `type: string`",
            ));
            return;
        }
        Some(Value::String(text)) if text.contains("${{") => return,
        Some(Value::String(text)) => {
            if !catalog::CALL_INPUT_TYPES.contains(&text.as_str()) {
                findings.push(error(
                    "preloop/invalid-dispatch-input",
                    &format!("{path}/type"),
                    format!(
                        "input `{id}` of `workflow_call` has unknown type `{text}`; expected one of {}",
                        list(catalog::CALL_INPUT_TYPES)
                    ),
                    "`workflow_call` inputs may only be `boolean`, `number`, or `string`",
                ));
                return;
            }
            Some(text.as_str())
        }
        Some(_) => {
            findings.push(error(
                "preloop/invalid-dispatch-input",
                &format!("{path}/type"),
                format!("`type` of input `{id}` must be a string"),
                "use `boolean`, `number`, or `string`",
            ));
            return;
        }
    };

    match input_type {
        Some("boolean") => check_boolean_default(id, input.get("default"), path, findings),
        Some("number") => check_number_default(id, input.get("default"), path, findings),
        _ => {}
    }
}

fn check_output(id: &str, output: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    match output {
        Value::Null => findings.push(error(
            "preloop/invalid-dispatch-input",
            path,
            format!("output `{id}` of `workflow_call` is missing `value`"),
            "add `value: ${{ jobs.<job>.outputs.<output> }}`",
        )),
        Value::Object(output) => match output.get("value") {
            None | Some(Value::Null) => findings.push(error(
                "preloop/invalid-dispatch-input",
                &format!("{path}/value"),
                format!("output `{id}` of `workflow_call` is missing `value`"),
                "add `value: ${{ jobs.<job>.outputs.<output> }}`",
            )),
            Some(Value::String(_)) => {}
            Some(_) => findings.push(error(
                "preloop/invalid-dispatch-input",
                &format!("{path}/value"),
                format!("`value` of output `{id}` must be a string expression"),
                "reference a job output with `${{ jobs.<job>.outputs.<output> }}`",
            )),
        },
        _ => findings.push(error(
            "preloop/invalid-dispatch-input",
            path,
            format!("output `{id}` of `workflow_call` must be a mapping"),
            "write the output as a mapping with `description` and `value`",
        )),
    }
}

fn check_secret(id: &str, secret: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    match secret {
        Value::Null => {}
        Value::Object(secret) => {
            if let Some(required) = secret.get("required") {
                match required {
                    Value::Bool(_) => {}
                    Value::String(text) if text.contains("${{") => {}
                    _ => findings.push(error(
                        "preloop/invalid-dispatch-input",
                        &format!("{path}/required"),
                        format!("`required` of secret `{id}` must be a boolean"),
                        "write `required: true` or `required: false`",
                    )),
                }
            }
            if let Some(description) = secret.get("description")
                && !description.is_string()
            {
                findings.push(error(
                    "preloop/invalid-dispatch-input",
                    &format!("{path}/description"),
                    format!("`description` of secret `{id}` must be a string"),
                    "describe the secret in a single string",
                ));
            }
        }
        _ => findings.push(error(
            "preloop/invalid-dispatch-input",
            path,
            format!("secret `{id}` of `workflow_call` must be a mapping of settings"),
            "write the secret as a mapping with `description` and `required`",
        )),
    }
}

fn check_input_id(id: &str, path: &str, findings: &mut Vec<Diagnostic>) {
    if !valid_input_id(id) {
        findings.push(error(
            "preloop/invalid-dispatch-input",
            path,
            format!(
                "input id `{id}` is not valid; ids must start with a letter or `_` and contain only letters, digits, `-`, and `_`"
            ),
            "rename the input to a GitHub-compatible identifier",
        ));
    }
}

fn valid_input_id(id: &str) -> bool {
    let mut characters = id.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn list<T: AsRef<str>>(values: &[T]) -> String {
    values
        .iter()
        .map(|value| format!("`{}`", value.as_ref()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{check_call, check_dispatch};

    fn dispatch(value: serde_json::Value) -> Vec<crate::Diagnostic> {
        let mut findings = Vec::new();
        check_dispatch(&value, "/on/workflow_dispatch", &mut findings);
        findings
    }

    fn call(value: serde_json::Value) -> Vec<crate::Diagnostic> {
        let mut findings = Vec::new();
        check_call(&value, "/on/workflow_call", &mut findings);
        findings
    }

    #[test]
    fn accepts_valid_dispatch_inputs() {
        let findings = dispatch(json!({
            "inputs": {
                "logLevel": {
                    "description": "Log level",
                    "required": true,
                    "default": "warning",
                    "type": "choice",
                    "options": ["info", "warning", "debug"]
                },
                "tags": { "type": "boolean", "default": false },
                "count": { "type": "number", "default": 3 },
                "environment": { "type": "environment" },
                "_private": { "type": "string", "default": "x" }
            }
        }));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn rejects_unknown_dispatch_input_type() {
        let findings = dispatch(json!({
            "inputs": { "profile": { "type": "select" } }
        }));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "preloop/invalid-dispatch-input");
        assert_eq!(
            findings[0].path,
            "/on/workflow_dispatch/inputs/profile/type"
        );
    }

    #[test]
    fn rejects_choice_without_options_or_default_outside_options() {
        let findings = dispatch(json!({
            "inputs": {
                "missing": { "type": "choice" },
                "outside": { "type": "choice", "options": ["a", "b"], "default": "c" }
            }
        }));
        let paths: Vec<&str> = findings
            .iter()
            .map(|finding| finding.path.as_str())
            .collect();
        assert!(paths.contains(&"/on/workflow_dispatch/inputs/missing/options"));
        assert!(paths.contains(&"/on/workflow_dispatch/inputs/outside/default"));
    }

    #[test]
    fn rejects_boolean_default_that_is_not_boolean() {
        let findings = dispatch(json!({
            "inputs": { "flag": { "type": "boolean", "default": "yes" } }
        }));
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].path,
            "/on/workflow_dispatch/inputs/flag/default"
        );
    }

    #[test]
    fn rejects_invalid_input_id() {
        let findings = dispatch(json!({
            "inputs": { "2fast": { "type": "string" } }
        }));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, "/on/workflow_dispatch/inputs/2fast");
    }

    #[test]
    fn expression_defaults_are_unknown_not_wrong() {
        let findings = dispatch(json!({
            "inputs": {
                "flag": { "type": "boolean", "default": "${{ inputs.other }}" },
                "count": { "type": "number", "default": "${{ inputs.other }}" }
            }
        }));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn accepts_valid_workflow_call_shape() {
        let findings = call(json!({
            "inputs": {
                "username": { "type": "string", "default": "john-doe" },
                "flag": { "type": "boolean", "default": false }
            },
            "outputs": {
                "workflow_output": {
                    "description": "A job output",
                    "value": "${{ jobs.my_job.outputs.job_output }}"
                }
            },
            "secrets": {
                "access-token": { "description": "A token", "required": false }
            }
        }));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn rejects_workflow_call_shape_errors() {
        let findings = call(json!({
            "inputs": {
                "noType": { "default": "x" },
                "badType": { "type": "choice" }
            },
            "outputs": {
                "noValue": { "description": "missing" }
            },
            "secrets": {
                "stringSecret": "not-a-mapping",
                "badRequired": { "required": "yes" }
            }
        }));
        let paths: Vec<&str> = findings
            .iter()
            .map(|finding| finding.path.as_str())
            .collect();
        assert!(paths.contains(&"/on/workflow_call/inputs/noType/type"));
        assert!(paths.contains(&"/on/workflow_call/inputs/badType/type"));
        assert!(paths.contains(&"/on/workflow_call/outputs/noValue/value"));
        assert!(paths.contains(&"/on/workflow_call/secrets/stringSecret"));
        assert!(paths.contains(&"/on/workflow_call/secrets/badRequired/required"));
    }
}
