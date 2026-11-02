package core

import (
	"github.com/preloopdev/loopt/sdk/go/expr"
	"github.com/preloopdev/loopt/sdk/go/om"
)

// WorkflowBuilder constructs a whole GitHub Actions workflow document.
// Field order on Build follows GitHub's conventional layout: name, on,
// permissions, env, concurrency, defaults, jobs.
type WorkflowBuilder struct {
	name        *string
	on          any
	permissions any
	env         []om.Field
	concurrency any
	defaults    any
	jobs        *om.Object
}

// NewWorkflow builds a workflow.
func NewWorkflow() *WorkflowBuilder {
	return &WorkflowBuilder{jobs: om.New()}
}

// Name sets `name:`.
func (b *WorkflowBuilder) Name(name string) *WorkflowBuilder {
	b.name = &name
	return b
}

// On sets `on:`.
func (b *WorkflowBuilder) On(on any) *WorkflowBuilder {
	b.on = expr.Value(on)
	return b
}

// Permissions sets `permissions:`.
func (b *WorkflowBuilder) Permissions(permissions any) *WorkflowBuilder {
	b.permissions = expr.Value(permissions)
	return b
}

// Env appends one `env:` entry, preserving call order.
func (b *WorkflowBuilder) Env(key string, value any) *WorkflowBuilder {
	b.env = setField(b.env, key, expr.Value(value))
	return b
}

// Concurrency sets `concurrency:`.
func (b *WorkflowBuilder) Concurrency(concurrency any) *WorkflowBuilder {
	b.concurrency = expr.Value(concurrency)
	return b
}

// Defaults sets `defaults:`.
func (b *WorkflowBuilder) Defaults(defaults any) *WorkflowBuilder {
	b.defaults = expr.Value(defaults)
	return b
}

// AddJob places job under jobs.<id>, in call order, returning a JobRef that
// later jobs can pass to Needs or use to build
// needs.<id>.outputs.<name> / .result expressions. Adding the same id again
// replaces that job's value in place.
func (b *WorkflowBuilder) AddJob(id string, job Job) *JobRef {
	b.jobs.Set(id, job.value)
	return &JobRef{
		id:              id,
		declaredOutputs: job.declaredOutputs,
		checkOutputs:    job.checkOutputs,
	}
}

// Build finishes the workflow document.
func (b *WorkflowBuilder) Build() *om.Object {
	fields := make([]om.Field, 0, 7)
	if b.name != nil {
		fields = append(fields, om.F("name", *b.name))
	}
	if b.on != nil {
		fields = append(fields, om.F("on", b.on))
	}
	if b.permissions != nil {
		fields = append(fields, om.F("permissions", b.permissions))
	}
	if len(b.env) > 0 {
		fields = append(fields, om.F("env", om.New(b.env...)))
	}
	if b.concurrency != nil {
		fields = append(fields, om.F("concurrency", b.concurrency))
	}
	if b.defaults != nil {
		fields = append(fields, om.F("defaults", b.defaults))
	}
	fields = append(fields, om.F("jobs", b.jobs))
	return om.New(fields...)
}
