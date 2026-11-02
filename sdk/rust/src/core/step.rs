//! A single GitHub Actions step, built from [`run`] or [`uses`].

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::expr::{steps_context, Expr};

#[derive(Debug, Clone)]
enum Action {
    Run(String),
    Uses(String),
}

/// Builds a step that runs a shell command (`run:`).
pub fn run(command: impl Into<String>) -> StepBuilder {
    StepBuilder::new(Action::Run(command.into()))
}

/// Builds a step that runs an action (`uses:`), including local (`./path`)
/// and Docker (`docker://image`) actions.
pub fn uses(action: impl Into<String>) -> StepBuilder {
    StepBuilder::new(Action::Uses(action.into()))
}

/// A step under construction. Field order on [`StepBuilder::build`] follows
/// GitHub's conventional layout: `id`, `if`, `name`, then the action or
/// command, its `with`/`env` inputs, and finally execution tweaks (`shell`,
/// `working-directory`, `continue-on-error`, `timeout-minutes`).
#[derive(Debug, Clone)]
pub struct StepBuilder {
    id: Option<String>,
    if_cond: Option<Expr>,
    name: Option<String>,
    action: Action,
    with: Vec<(String, Value)>,
    env: Vec<(String, Value)>,
    shell: Option<String>,
    working_directory: Option<String>,
    continue_on_error: Option<bool>,
    timeout_minutes: Option<i64>,
}

impl StepBuilder {
    fn new(action: Action) -> StepBuilder {
        StepBuilder {
            id: None,
            if_cond: None,
            name: None,
            action,
            with: Vec::new(),
            env: Vec::new(),
            shell: None,
            working_directory: None,
            continue_on_error: None,
            timeout_minutes: None,
        }
    }

    /// Sets `id:`, required to later reference this step's outputs,
    /// outcome, or conclusion via the built [`StepRef`].
    pub fn id(mut self, id: impl Into<String>) -> StepBuilder {
        self.id = Some(id.into());
        self
    }

    /// Sets `name:`.
    pub fn name(mut self, name: impl Into<String>) -> StepBuilder {
        self.name = Some(name.into());
        self
    }

    /// Sets `if:`.
    pub fn if_cond(mut self, condition: impl Into<Expr>) -> StepBuilder {
        self.if_cond = Some(condition.into());
        self
    }

    /// Appends one `with:` entry, preserving call order.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> StepBuilder {
        self.with.push((key.into(), value.into()));
        self
    }

    /// Appends one `env:` entry, preserving call order.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<Value>) -> StepBuilder {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Sets `shell:`.
    pub fn shell(mut self, shell: impl Into<String>) -> StepBuilder {
        self.shell = Some(shell.into());
        self
    }

    /// Sets `working-directory:`.
    pub fn working_directory(mut self, directory: impl Into<String>) -> StepBuilder {
        self.working_directory = Some(directory.into());
        self
    }

    /// Sets `continue-on-error:`.
    pub fn continue_on_error(mut self, value: bool) -> StepBuilder {
        self.continue_on_error = Some(value);
        self
    }

    /// Sets `timeout-minutes:`.
    pub fn timeout_minutes(mut self, minutes: i64) -> StepBuilder {
        self.timeout_minutes = Some(minutes);
        self
    }

    /// Finishes the step, producing its JSON value and a typed reference to
    /// its outputs/outcome/conclusion (available once `.id(...)` is set).
    pub fn build(self) -> StepRef {
        let mut map = Map::new();
        if let Some(id) = &self.id {
            map.insert("id".to_string(), Value::String(id.clone()));
        }
        if let Some(condition) = &self.if_cond {
            map.insert("if".to_string(), Value::from(condition));
        }
        if let Some(name) = &self.name {
            map.insert("name".to_string(), Value::String(name.clone()));
        }
        match self.action {
            Action::Uses(action) => {
                map.insert("uses".to_string(), Value::String(action));
            }
            Action::Run(command) => {
                map.insert("run".to_string(), Value::String(command));
            }
        }
        if !self.with.is_empty() {
            let mut with = Map::new();
            for (key, value) in self.with {
                with.insert(key, value);
            }
            map.insert("with".to_string(), Value::Object(with));
        }
        if let Some(shell) = self.shell {
            map.insert("shell".to_string(), Value::String(shell));
        }
        if let Some(directory) = self.working_directory {
            map.insert("working-directory".to_string(), Value::String(directory));
        }
        if !self.env.is_empty() {
            let mut env = Map::new();
            for (key, value) in self.env {
                env.insert(key, value);
            }
            map.insert("env".to_string(), Value::Object(env));
        }
        if let Some(value) = self.continue_on_error {
            map.insert("continue-on-error".to_string(), Value::Bool(value));
        }
        if let Some(minutes) = self.timeout_minutes {
            map.insert("timeout-minutes".to_string(), Value::Number(minutes.into()));
        }
        StepRef {
            value: Value::Object(map),
            id: self.id,
        }
    }
}

/// A finished step, ready to place in a job's `steps` list via
/// [`crate::core::JobBuilder::step`], with typed access to
/// `steps.<id>.outputs.<name>` / `.outcome` / `.conclusion` once it carries
/// an explicit `id`.
#[derive(Debug, Clone)]
pub struct StepRef {
    value: Value,
    id: Option<String>,
}

impl StepRef {
    /// The step's JSON value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Consumes the reference, returning its JSON value.
    pub fn into_value(self) -> Value {
        self.value
    }

    /// The explicit `id` this step carries, if any.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// `steps.<id>.outputs.<name>`.
    ///
    /// # Errors
    /// Returns [`Error::MissingStepId`] when this step has no `id`.
    pub fn output(&self, name: &str) -> Result<Expr> {
        let id = self.require_id()?;
        Ok(steps_context()
            .field(id)
            .field("outputs")
            .field(name)
            .into_expr())
    }

    /// `steps.<id>.outcome`: the step's result before `continue-on-error`
    /// is applied.
    ///
    /// # Errors
    /// Returns [`Error::MissingStepId`] when this step has no `id`.
    pub fn outcome(&self) -> Result<Expr> {
        let id = self.require_id()?;
        Ok(steps_context().field(id).field("outcome").into_expr())
    }

    /// `steps.<id>.conclusion`: the step's result after
    /// `continue-on-error` is applied.
    ///
    /// # Errors
    /// Returns [`Error::MissingStepId`] when this step has no `id`.
    pub fn conclusion(&self) -> Result<Expr> {
        let id = self.require_id()?;
        Ok(steps_context().field(id).field("conclusion").into_expr())
    }

    fn require_id(&self) -> Result<&str> {
        self.id.as_deref().ok_or(Error::MissingStepId)
    }
}
