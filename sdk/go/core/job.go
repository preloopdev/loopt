package core

import (
	"github.com/preloopdev/loopt/sdk/go/expr"
	"github.com/preloopdev/loopt/sdk/go/om"
)

// JobRef is a job placed under a workflow's jobs map, carrying the id it
// was placed under and, for a regular job, the output names it declared, so
// Output can validate a reference against them.
type JobRef struct {
	id              string
	declaredOutputs []string
	checkOutputs    bool
}

// ID returns the id this job was placed under in its workflow.
func (r *JobRef) ID() string {
	return r.id
}

// Output builds `needs.<id>.outputs.<name>`. For a regular job it returns
// an *UndeclaredOutputError when name is not one of the job's declared
// outputs; a reusable-workflow call's outputs are whatever the called
// workflow declares, so they are never checked.
func (r *JobRef) Output(name string) (expr.Expr, error) {
	if r.checkOutputs {
		declared := false
		for _, output := range r.declaredOutputs {
			if output == name {
				declared = true
				break
			}
		}
		if !declared {
			return expr.Expr{}, &UndeclaredOutputError{
				Kind:     "job",
				Name:     name,
				Declared: append([]string(nil), r.declaredOutputs...),
			}
		}
	}
	return expr.Needs().Field(r.id).Field("outputs").Field(name).Expr(), nil
}

// Result builds `needs.<id>.result`.
func (r *JobRef) Result() expr.Expr {
	return expr.Needs().Field(r.id).Field("result").Expr()
}

// Job is a finished job, ready to place under a workflow's jobs map via
// WorkflowBuilder.AddJob.
type Job struct {
	value           *om.Object
	declaredOutputs []string
	checkOutputs    bool
}

// Value returns the job's ordered JSON object.
func (j Job) Value() *om.Object {
	return j.value
}

// JobBuilder constructs a regular job (`runs-on:`). Field order on Build
// follows GitHub's conventional layout: name, needs, if, permissions,
// runs-on, environment, container, services, defaults, continue-on-error,
// strategy, timeout-minutes, concurrency, env, outputs, steps.
type JobBuilder struct {
	name            *string
	needs           []*JobRef
	ifCond          any
	permissions     any
	runsOn          any
	environment     any
	container       any
	services        any
	defaults        any
	continueOnError *bool
	strategy        any
	timeout         *int64
	concurrency     any
	env             []om.Field
	outputs         []om.Field
	steps           []StepRef
}

// NewJob builds a regular job.
func NewJob() *JobBuilder {
	return &JobBuilder{}
}

// Name sets `name:`.
func (b *JobBuilder) Name(name string) *JobBuilder {
	b.name = &name
	return b
}

// Needs sets `needs:` to the ids of refs, in order.
func (b *JobBuilder) Needs(refs ...*JobRef) *JobBuilder {
	b.needs = append([]*JobRef(nil), refs...)
	return b
}

// If sets `if:`.
func (b *JobBuilder) If(condition any) *JobBuilder {
	b.ifCond = expr.Value(condition)
	return b
}

// Permissions sets `permissions:`.
func (b *JobBuilder) Permissions(permissions any) *JobBuilder {
	b.permissions = expr.Value(permissions)
	return b
}

// RunsOn sets `runs-on:`.
func (b *JobBuilder) RunsOn(runner any) *JobBuilder {
	b.runsOn = expr.Value(runner)
	return b
}

// Environment sets `environment:` (a name, or an object with `name` and
// `url`).
func (b *JobBuilder) Environment(environment any) *JobBuilder {
	b.environment = expr.Value(environment)
	return b
}

// Container sets `container:`.
func (b *JobBuilder) Container(container any) *JobBuilder {
	b.container = expr.Value(container)
	return b
}

// Services sets `services:`.
func (b *JobBuilder) Services(services any) *JobBuilder {
	b.services = expr.Value(services)
	return b
}

// Defaults sets `defaults:` (including its `run:` sub-key).
func (b *JobBuilder) Defaults(defaults any) *JobBuilder {
	b.defaults = expr.Value(defaults)
	return b
}

// ContinueOnError sets `continue-on-error:`.
func (b *JobBuilder) ContinueOnError(value bool) *JobBuilder {
	b.continueOnError = &value
	return b
}

// Strategy sets `strategy:` (including its `matrix:` sub-key).
func (b *JobBuilder) Strategy(strategy any) *JobBuilder {
	b.strategy = expr.Value(strategy)
	return b
}

// TimeoutMinutes sets `timeout-minutes:`.
func (b *JobBuilder) TimeoutMinutes(minutes int64) *JobBuilder {
	b.timeout = &minutes
	return b
}

// Concurrency sets `concurrency:` (a group name, or an object with `group`
// and `cancel-in-progress`).
func (b *JobBuilder) Concurrency(concurrency any) *JobBuilder {
	b.concurrency = expr.Value(concurrency)
	return b
}

// Env appends one `env:` entry, preserving call order.
func (b *JobBuilder) Env(key string, value any) *JobBuilder {
	b.env = setField(b.env, key, expr.Value(value))
	return b
}

// Output appends one `outputs:` entry, preserving call order. Declaring an
// output name enables JobRef.Output to reference it.
func (b *JobBuilder) Output(name string, value any) *JobBuilder {
	b.outputs = setField(b.outputs, name, expr.Value(value))
	return b
}

// Step appends one step to `steps:`.
func (b *JobBuilder) Step(step StepRef) *JobBuilder {
	b.steps = append(b.steps, step)
	return b
}

// Build finishes the job.
func (b *JobBuilder) Build() Job {
	fields := make([]om.Field, 0, 16)
	if b.name != nil {
		fields = append(fields, om.F("name", *b.name))
	}
	if len(b.needs) > 0 {
		fields = append(fields, om.F("needs", renderNeeds(b.needs)))
	}
	if b.ifCond != nil {
		fields = append(fields, om.F("if", b.ifCond))
	}
	if b.permissions != nil {
		fields = append(fields, om.F("permissions", b.permissions))
	}
	if b.runsOn != nil {
		fields = append(fields, om.F("runs-on", b.runsOn))
	}
	if b.environment != nil {
		fields = append(fields, om.F("environment", b.environment))
	}
	if b.container != nil {
		fields = append(fields, om.F("container", b.container))
	}
	if b.services != nil {
		fields = append(fields, om.F("services", b.services))
	}
	if b.defaults != nil {
		fields = append(fields, om.F("defaults", b.defaults))
	}
	if b.continueOnError != nil {
		fields = append(fields, om.F("continue-on-error", *b.continueOnError))
	}
	if b.strategy != nil {
		fields = append(fields, om.F("strategy", b.strategy))
	}
	if b.timeout != nil {
		fields = append(fields, om.F("timeout-minutes", *b.timeout))
	}
	if b.concurrency != nil {
		fields = append(fields, om.F("concurrency", b.concurrency))
	}
	if len(b.env) > 0 {
		fields = append(fields, om.F("env", om.New(b.env...)))
	}
	declaredOutputs := make([]string, len(b.outputs))
	for i, field := range b.outputs {
		declaredOutputs[i] = field.Key
	}
	if len(b.outputs) > 0 {
		fields = append(fields, om.F("outputs", om.New(b.outputs...)))
	}
	steps := make([]any, len(b.steps))
	for i, step := range b.steps {
		steps[i] = step.Value()
	}
	fields = append(fields, om.F("steps", steps))
	return Job{
		value:           om.New(fields...),
		declaredOutputs: declaredOutputs,
		checkOutputs:    true,
	}
}

// CallJobBuilder constructs a reusable-workflow caller job (`uses:`). Field
// order on Build follows GitHub's conventional layout: name, needs, if,
// permissions, uses, with, secrets, strategy, concurrency.
type CallJobBuilder struct {
	name           *string
	needs          []*JobRef
	ifCond         any
	permissions    any
	uses           string
	with           []om.Field
	secrets        []om.Field
	secretsInherit bool
	strategy       any
	concurrency    any
}

// NewCallJob builds a reusable-workflow caller job.
func NewCallJob(uses string) *CallJobBuilder {
	return &CallJobBuilder{uses: uses}
}

// Name sets `name:`.
func (b *CallJobBuilder) Name(name string) *CallJobBuilder {
	b.name = &name
	return b
}

// Needs sets `needs:` to the ids of refs, in order.
func (b *CallJobBuilder) Needs(refs ...*JobRef) *CallJobBuilder {
	b.needs = append([]*JobRef(nil), refs...)
	return b
}

// If sets `if:`.
func (b *CallJobBuilder) If(condition any) *CallJobBuilder {
	b.ifCond = expr.Value(condition)
	return b
}

// Permissions sets `permissions:`.
func (b *CallJobBuilder) Permissions(permissions any) *CallJobBuilder {
	b.permissions = expr.Value(permissions)
	return b
}

// With appends one `with:` entry, preserving call order.
func (b *CallJobBuilder) With(key string, value any) *CallJobBuilder {
	b.with = setField(b.with, key, expr.Value(value))
	return b
}

// Secrets appends one `secrets:` entry, preserving call order.
func (b *CallJobBuilder) Secrets(key string, value any) *CallJobBuilder {
	b.secrets = setField(b.secrets, key, expr.Value(value))
	return b
}

// SecretsInherit sets `secrets: inherit`, passing all of the calling
// workflow's secrets to the called workflow. It cannot be combined with
// explicit Secrets entries; Build returns ErrSecretsInheritConflict if both
// are set.
func (b *CallJobBuilder) SecretsInherit() *CallJobBuilder {
	b.secretsInherit = true
	return b
}

// Strategy sets `strategy:` (including its `matrix:` sub-key).
func (b *CallJobBuilder) Strategy(strategy any) *CallJobBuilder {
	b.strategy = expr.Value(strategy)
	return b
}

// Concurrency sets `concurrency:` (a group name, or an object with `group`
// and `cancel-in-progress`).
func (b *CallJobBuilder) Concurrency(concurrency any) *CallJobBuilder {
	b.concurrency = expr.Value(concurrency)
	return b
}

// Build finishes the job. Its outputs are never declared, so JobRef.Output
// accepts any name for the returned reference. It returns
// ErrSecretsInheritConflict when both SecretsInherit and explicit Secrets
// entries were set.
func (b *CallJobBuilder) Build() (Job, error) {
	if b.secretsInherit && len(b.secrets) > 0 {
		return Job{}, ErrSecretsInheritConflict
	}
	fields := make([]om.Field, 0, 9)
	if b.name != nil {
		fields = append(fields, om.F("name", *b.name))
	}
	if len(b.needs) > 0 {
		fields = append(fields, om.F("needs", renderNeeds(b.needs)))
	}
	if b.ifCond != nil {
		fields = append(fields, om.F("if", b.ifCond))
	}
	if b.permissions != nil {
		fields = append(fields, om.F("permissions", b.permissions))
	}
	fields = append(fields, om.F("uses", b.uses))
	if len(b.with) > 0 {
		fields = append(fields, om.F("with", om.New(b.with...)))
	}
	if b.secretsInherit {
		fields = append(fields, om.F("secrets", "inherit"))
	} else if len(b.secrets) > 0 {
		fields = append(fields, om.F("secrets", om.New(b.secrets...)))
	}
	if b.strategy != nil {
		fields = append(fields, om.F("strategy", b.strategy))
	}
	if b.concurrency != nil {
		fields = append(fields, om.F("concurrency", b.concurrency))
	}
	return Job{value: om.New(fields...)}, nil
}

func renderNeeds(refs []*JobRef) any {
	ids := make([]any, len(refs))
	for i, ref := range refs {
		ids[i] = ref.ID()
	}
	if len(ids) == 1 {
		return ids[0]
	}
	return ids
}
