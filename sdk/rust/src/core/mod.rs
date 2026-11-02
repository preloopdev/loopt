//! Workflow authoring core: hand-written `run`/`uses` step, `job`, and
//! `workflow` constructors for the fields real workflows use, plus typed
//! job/step references. Mirrors the shape (not the schema-generated
//! internals) of the TypeScript SDK's `src/core/`.

mod job;
mod step;
mod workflow;

pub use job::{call_workflow, job, CallJobBuilder, Job, JobBuilder, JobRef};
pub use step::{run, uses, StepBuilder, StepRef};
pub use workflow::{workflow, WorkflowBuilder};
