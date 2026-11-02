//! A single GitHub Actions job: a regular job built from [`job`], or a
//! reusable-workflow caller job built from [`call_workflow`].

use serde_json::{Map, Value};

use super::step::StepRef;
use crate::error::{Error, Result};
use crate::expr::{needs_context, Expr};

/// A job placed under a workflow's `jobs:` map, carrying the id it was
/// placed under and (for a regular job) the output names it declared, so
/// [`JobRef::output`] can validate a reference against them.
#[derive(Debug, Clone)]
pub struct JobRef {
    id: String,
    /// `None` for a reusable-workflow call, whose outputs are whatever the
    /// called workflow happens to declare and so are never checked here
    /// (matching the TypeScript SDK's `JobRef`); `Some(declared)` for a
    /// regular job, restricted to its own `outputs:` mapping.
    declared_outputs: Option<Vec<String>>,
}

impl JobRef {
    pub(crate) fn new(id: String, declared_outputs: Option<Vec<String>>) -> JobRef {
        JobRef {
            id,
            declared_outputs,
        }
    }

    /// The id this job was placed under in its workflow.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `needs.<id>.outputs.<name>`.
    ///
    /// # Errors
    /// Returns [`Error::UndeclaredOutput`] when this is a regular job and
    /// `name` is not one of its declared outputs.
    pub fn output(&self, name: &str) -> Result<Expr> {
        if let Some(declared) = &self.declared_outputs {
            if !declared.iter().any(|output| output == name) {
                return Err(Error::UndeclaredOutput {
                    kind: "job",
                    name: name.to_string(),
                    declared: declared.clone(),
                });
            }
        }
        Ok(needs_context()
            .field(&self.id)
            .field("outputs")
            .field(name)
            .into_expr())
    }

    /// `needs.<id>.result`.
    pub fn result(&self) -> Expr {
        needs_context().field(&self.id).field("result").into_expr()
    }
}

impl From<&JobRef> for Value {
    fn from(job_ref: &JobRef) -> Value {
        Value::String(job_ref.id.clone())
    }
}

/// A finished job, ready to place under a workflow's `jobs:` map via
/// [`crate::core::WorkflowBuilder::job`].
#[derive(Debug, Clone)]
pub struct Job {
    pub(crate) value: Value,
    pub(crate) declared_outputs: Option<Vec<String>>,
}

impl Job {
    /// The job's JSON value.
    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Builds a regular job (`runs-on:`), via [`JobBuilder`].
pub fn job() -> JobBuilder {
    JobBuilder::new()
}

/// A regular job under construction. Field order on [`JobBuilder::build`]
/// follows GitHub's conventional layout: `name`, `needs`, `if`, `runs-on`,
/// `strategy`, `timeout-minutes`, `env`, `outputs`, `steps`.
#[derive(Debug, Clone, Default)]
pub struct JobBuilder {
    name: Option<String>,
    needs: Vec<String>,
    if_cond: Option<Expr>,
    runs_on: Option<Value>,
    strategy: Option<Value>,
    timeout_minutes: Option<i64>,
    env: Vec<(String, Value)>,
    outputs: Vec<(String, Value)>,
    steps: Vec<StepRef>,
}

impl JobBuilder {
    fn new() -> JobBuilder {
        JobBuilder::default()
    }

    /// Sets `name:`.
    pub fn name(mut self, name: impl Into<String>) -> JobBuilder {
        self.name = Some(name.into());
        self
    }

    /// Sets `needs:` to the ids of `refs`, in order.
    pub fn needs(mut self, refs: &[&JobRef]) -> JobBuilder {
        self.needs = refs.iter().map(|job_ref| job_ref.id.clone()).collect();
        self
    }

    /// Sets `if:`.
    pub fn if_cond(mut self, condition: impl Into<Expr>) -> JobBuilder {
        self.if_cond = Some(condition.into());
        self
    }

    /// Sets `runs-on:`.
    pub fn runs_on(mut self, runner: impl Into<Value>) -> JobBuilder {
        self.runs_on = Some(runner.into());
        self
    }

    /// Sets `strategy:` (including its `matrix:` sub-key).
    pub fn strategy(mut self, strategy: impl Into<Value>) -> JobBuilder {
        self.strategy = Some(strategy.into());
        self
    }

    /// Sets `timeout-minutes:`.
    pub fn timeout_minutes(mut self, minutes: i64) -> JobBuilder {
        self.timeout_minutes = Some(minutes);
        self
    }

    /// Appends one `env:` entry, preserving call order.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<Value>) -> JobBuilder {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Appends one `outputs:` entry, preserving call order. Declares the
    /// output name, enabling [`JobRef::output`] to reference it.
    pub fn output(mut self, name: impl Into<String>, value: impl Into<Value>) -> JobBuilder {
        self.outputs.push((name.into(), value.into()));
        self
    }

    /// Appends one step to `steps:`.
    pub fn step(mut self, step: StepRef) -> JobBuilder {
        self.steps.push(step);
        self
    }

    /// Finishes the job.
    pub fn build(self) -> Job {
        let mut map = Map::new();
        if let Some(name) = self.name {
            map.insert("name".to_string(), Value::String(name));
        }
        if !self.needs.is_empty() {
            let needs = if self.needs.len() == 1 {
                Value::String(self.needs.into_iter().next().expect("checked len == 1"))
            } else {
                Value::Array(self.needs.into_iter().map(Value::String).collect())
            };
            map.insert("needs".to_string(), needs);
        }
        if let Some(condition) = self.if_cond {
            map.insert("if".to_string(), Value::from(condition));
        }
        if let Some(runs_on) = self.runs_on {
            map.insert("runs-on".to_string(), runs_on);
        }
        if let Some(strategy) = self.strategy {
            map.insert("strategy".to_string(), strategy);
        }
        if let Some(minutes) = self.timeout_minutes {
            map.insert("timeout-minutes".to_string(), Value::Number(minutes.into()));
        }
        if !self.env.is_empty() {
            let mut env = Map::new();
            for (key, value) in self.env {
                env.insert(key, value);
            }
            map.insert("env".to_string(), Value::Object(env));
        }
        let declared_outputs = self
            .outputs
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if !self.outputs.is_empty() {
            let mut outputs = Map::new();
            for (name, value) in self.outputs {
                outputs.insert(name, value);
            }
            map.insert("outputs".to_string(), Value::Object(outputs));
        }
        map.insert(
            "steps".to_string(),
            Value::Array(self.steps.into_iter().map(StepRef::into_value).collect()),
        );
        Job {
            value: Value::Object(map),
            declared_outputs: Some(declared_outputs),
        }
    }
}

/// Builds a reusable-workflow caller job (`uses:`), via [`CallJobBuilder`].
pub fn call_workflow(uses: impl Into<String>) -> CallJobBuilder {
    CallJobBuilder::new(uses.into())
}

/// A reusable-workflow caller job under construction. Field order on
/// [`CallJobBuilder::build`] follows GitHub's conventional layout: `name`,
/// `needs`, `if`, `uses`, `with`, `secrets`, `strategy`.
#[derive(Debug, Clone)]
pub struct CallJobBuilder {
    name: Option<String>,
    needs: Vec<String>,
    if_cond: Option<Expr>,
    uses: String,
    with: Vec<(String, Value)>,
    secrets: Vec<(String, Value)>,
    strategy: Option<Value>,
}

impl CallJobBuilder {
    fn new(uses: String) -> CallJobBuilder {
        CallJobBuilder {
            name: None,
            needs: Vec::new(),
            if_cond: None,
            uses,
            with: Vec::new(),
            secrets: Vec::new(),
            strategy: None,
        }
    }

    /// Sets `name:`.
    pub fn name(mut self, name: impl Into<String>) -> CallJobBuilder {
        self.name = Some(name.into());
        self
    }

    /// Sets `needs:` to the ids of `refs`, in order.
    pub fn needs(mut self, refs: &[&JobRef]) -> CallJobBuilder {
        self.needs = refs.iter().map(|job_ref| job_ref.id.clone()).collect();
        self
    }

    /// Sets `if:`.
    pub fn if_cond(mut self, condition: impl Into<Expr>) -> CallJobBuilder {
        self.if_cond = Some(condition.into());
        self
    }

    /// Appends one `with:` entry, preserving call order.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> CallJobBuilder {
        self.with.push((key.into(), value.into()));
        self
    }

    /// Appends one `secrets:` entry, preserving call order.
    pub fn secrets(mut self, key: impl Into<String>, value: impl Into<Value>) -> CallJobBuilder {
        self.secrets.push((key.into(), value.into()));
        self
    }

    /// Sets `strategy:` (including its `matrix:` sub-key).
    pub fn strategy(mut self, strategy: impl Into<Value>) -> CallJobBuilder {
        self.strategy = Some(strategy.into());
        self
    }

    /// Finishes the job. Its outputs are never declared, so
    /// [`JobRef::output`] accepts any name for the returned reference,
    /// matching the TypeScript SDK's reusable-workflow calls.
    pub fn build(self) -> Job {
        let mut map = Map::new();
        if let Some(name) = self.name {
            map.insert("name".to_string(), Value::String(name));
        }
        if !self.needs.is_empty() {
            let needs = if self.needs.len() == 1 {
                Value::String(self.needs.into_iter().next().expect("checked len == 1"))
            } else {
                Value::Array(self.needs.into_iter().map(Value::String).collect())
            };
            map.insert("needs".to_string(), needs);
        }
        if let Some(condition) = self.if_cond {
            map.insert("if".to_string(), Value::from(condition));
        }
        map.insert("uses".to_string(), Value::String(self.uses));
        if !self.with.is_empty() {
            let mut with = Map::new();
            for (key, value) in self.with {
                with.insert(key, value);
            }
            map.insert("with".to_string(), Value::Object(with));
        }
        if !self.secrets.is_empty() {
            let mut secrets = Map::new();
            for (key, value) in self.secrets {
                secrets.insert(key, value);
            }
            map.insert("secrets".to_string(), Value::Object(secrets));
        }
        if let Some(strategy) = self.strategy {
            map.insert("strategy".to_string(), strategy);
        }
        Job {
            value: Value::Object(map),
            declared_outputs: None,
        }
    }
}
