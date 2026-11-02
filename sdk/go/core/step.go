package core

import (
	"github.com/preloopdev/loopt/sdk/go/expr"
	"github.com/preloopdev/loopt/sdk/go/om"
)

type actionKind int

const (
	actionRun actionKind = iota
	actionUses
)

// StepBuilder constructs a single GitHub Actions step. Field order on Build
// follows GitHub's conventional layout: id, if, name, then the action or
// command, its with/env inputs, and finally execution tweaks (shell,
// working-directory, continue-on-error, timeout-minutes).
type StepBuilder struct {
	hasID      bool
	id         string
	ifCond     any
	name       *string
	kind       actionKind
	target     string
	with       []om.Field
	env        []om.Field
	shell      *string
	workingDir *string
	continueOn *bool
	timeout    *int64
}

// Run builds a step that runs a shell command (`run:`).
func Run(command string) *StepBuilder {
	return &StepBuilder{kind: actionRun, target: command}
}

// Uses builds a step that runs an action (`uses:`), including local
// (`./path`) and Docker (`docker://image`) actions.
func Uses(action string) *StepBuilder {
	return &StepBuilder{kind: actionUses, target: action}
}

// ID sets `id:`, required to later reference this step's outputs, outcome,
// or conclusion through the built StepRef.
func (b *StepBuilder) ID(id string) *StepBuilder {
	b.hasID = true
	b.id = id
	return b
}

// Name sets `name:`.
func (b *StepBuilder) Name(name string) *StepBuilder {
	b.name = &name
	return b
}

// If sets `if:`.
func (b *StepBuilder) If(condition any) *StepBuilder {
	b.ifCond = expr.Value(condition)
	return b
}

// With appends one `with:` entry, preserving call order.
func (b *StepBuilder) With(key string, value any) *StepBuilder {
	b.with = setField(b.with, key, expr.Value(value))
	return b
}

// Env appends one `env:` entry, preserving call order.
func (b *StepBuilder) Env(key string, value any) *StepBuilder {
	b.env = setField(b.env, key, expr.Value(value))
	return b
}

// Shell sets `shell:`.
func (b *StepBuilder) Shell(shell string) *StepBuilder {
	b.shell = &shell
	return b
}

// WorkingDirectory sets `working-directory:`.
func (b *StepBuilder) WorkingDirectory(directory string) *StepBuilder {
	b.workingDir = &directory
	return b
}

// ContinueOnError sets `continue-on-error:`.
func (b *StepBuilder) ContinueOnError(value bool) *StepBuilder {
	b.continueOn = &value
	return b
}

// TimeoutMinutes sets `timeout-minutes:`.
func (b *StepBuilder) TimeoutMinutes(minutes int64) *StepBuilder {
	b.timeout = &minutes
	return b
}

// Build finishes the step.
func (b *StepBuilder) Build() StepRef {
	fields := make([]om.Field, 0, 9)
	if b.hasID {
		fields = append(fields, om.F("id", b.id))
	}
	if b.ifCond != nil {
		fields = append(fields, om.F("if", b.ifCond))
	}
	if b.name != nil {
		fields = append(fields, om.F("name", *b.name))
	}
	if b.kind == actionUses {
		fields = append(fields, om.F("uses", b.target))
	} else {
		fields = append(fields, om.F("run", b.target))
	}
	if len(b.with) > 0 {
		fields = append(fields, om.F("with", om.New(b.with...)))
	}
	if b.shell != nil {
		fields = append(fields, om.F("shell", *b.shell))
	}
	if b.workingDir != nil {
		fields = append(fields, om.F("working-directory", *b.workingDir))
	}
	if len(b.env) > 0 {
		fields = append(fields, om.F("env", om.New(b.env...)))
	}
	if b.continueOn != nil {
		fields = append(fields, om.F("continue-on-error", *b.continueOn))
	}
	if b.timeout != nil {
		fields = append(fields, om.F("timeout-minutes", *b.timeout))
	}
	return StepRef{
		value: om.New(fields...),
		id:    b.id,
		hasID: b.hasID,
	}
}

// StepRef is a finished step, ready to place in a job's steps list via
// JobBuilder.Step, with typed access to steps.<id>.outputs.<name>,
// .outcome, and .conclusion once it carries an explicit ID.
type StepRef struct {
	value *om.Object
	id    string
	hasID bool
}

// Value returns the step's ordered JSON object.
func (s StepRef) Value() *om.Object {
	return s.value
}

// ID returns the explicit id this step carries, if any.
func (s StepRef) ID() (string, bool) {
	return s.id, s.hasID
}

// Output builds `steps.<id>.outputs.<name>`.
func (s StepRef) Output(name string) (expr.Expr, error) {
	if !s.hasID {
		return expr.Expr{}, ErrMissingStepID
	}
	return expr.Steps().Field(s.id).Field("outputs").Field(name).Expr(), nil
}

// Outcome builds `steps.<id>.outcome`: the step's result before
// continue-on-error is applied.
func (s StepRef) Outcome() (expr.Expr, error) {
	if !s.hasID {
		return expr.Expr{}, ErrMissingStepID
	}
	return expr.Steps().Field(s.id).Field("outcome").Expr(), nil
}

// Conclusion builds `steps.<id>.conclusion`: the step's result after
// continue-on-error is applied.
func (s StepRef) Conclusion() (expr.Expr, error) {
	if !s.hasID {
		return expr.Expr{}, ErrMissingStepID
	}
	return expr.Steps().Field(s.id).Field("conclusion").Expr(), nil
}
