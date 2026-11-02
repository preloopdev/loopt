//! A whole GitHub Actions workflow document, built from [`workflow`].

use serde_json::{Map, Value};

use super::job::{Job, JobRef};

/// Builds a workflow, via [`WorkflowBuilder`].
pub fn workflow() -> WorkflowBuilder {
    WorkflowBuilder::new()
}

/// A workflow under construction. Field order on [`WorkflowBuilder::build`]
/// follows GitHub's conventional layout: `name`, `run-name`, `on`,
/// `permissions`, `env`, `concurrency`, `defaults`, `jobs`.
#[derive(Debug, Clone, Default)]
pub struct WorkflowBuilder {
    name: Option<String>,
    run_name: Option<Value>,
    on: Option<Value>,
    permissions: Option<Value>,
    env: Vec<(String, Value)>,
    concurrency: Option<Value>,
    defaults: Option<Value>,
    jobs: Map<String, Value>,
}

impl WorkflowBuilder {
    fn new() -> WorkflowBuilder {
        WorkflowBuilder::default()
    }

    /// Sets `name:`.
    pub fn name(mut self, name: impl Into<String>) -> WorkflowBuilder {
        self.name = Some(name.into());
        self
    }

    /// Sets `on:`.
    pub fn on(mut self, on: impl Into<Value>) -> WorkflowBuilder {
        self.on = Some(on.into());
        self
    }

    /// Sets `run-name:` — the display name for the workflow run.
    pub fn run_name(mut self, run_name: impl Into<Value>) -> WorkflowBuilder {
        self.run_name = Some(run_name.into());
        self
    }

    /// Sets `permissions:`.
    pub fn permissions(mut self, permissions: impl Into<Value>) -> WorkflowBuilder {
        self.permissions = Some(permissions.into());
        self
    }

    /// Appends one `env:` entry, preserving call order.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<Value>) -> WorkflowBuilder {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Sets `concurrency:`.
    pub fn concurrency(mut self, concurrency: impl Into<Value>) -> WorkflowBuilder {
        self.concurrency = Some(concurrency.into());
        self
    }

    /// Sets `defaults:`.
    pub fn defaults(mut self, defaults: impl Into<Value>) -> WorkflowBuilder {
        self.defaults = Some(defaults.into());
        self
    }

    /// Places `job` under `jobs.<id>`, in call order, returning a
    /// [`JobRef`] later calls can pass to [`super::JobBuilder::needs`] or
    /// use to build `needs.<id>.outputs.<name>` / `.result` expressions.
    pub fn job(mut self, id: impl Into<String>, job: Job) -> (WorkflowBuilder, JobRef) {
        let id = id.into();
        let job_ref = JobRef::new(id.clone(), job.declared_outputs);
        self.jobs.insert(id, job.value);
        (self, job_ref)
    }

    /// Finishes the workflow document.
    pub fn build(self) -> Value {
        let mut map = Map::new();
        if let Some(name) = self.name {
            map.insert("name".to_string(), Value::String(name));
        }
        if let Some(run_name) = self.run_name {
            map.insert("run-name".to_string(), run_name);
        }
        if let Some(on) = self.on {
            map.insert("on".to_string(), on);
        }
        if let Some(permissions) = self.permissions {
            map.insert("permissions".to_string(), permissions);
        }
        if !self.env.is_empty() {
            let mut env = Map::new();
            for (key, value) in self.env {
                env.insert(key, value);
            }
            map.insert("env".to_string(), Value::Object(env));
        }
        if let Some(concurrency) = self.concurrency {
            map.insert("concurrency".to_string(), concurrency);
        }
        if let Some(defaults) = self.defaults {
            map.insert("defaults".to_string(), defaults);
        }
        map.insert("jobs".to_string(), Value::Object(self.jobs));
        Value::Object(map)
    }
}
