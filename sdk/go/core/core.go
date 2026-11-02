// Package core builds GitHub Actions steps, jobs, and whole workflow
// documents from Go values.
//
// Every constructor returns an ordered object from the SDK's om package
// rather than a plain map, so a document keeps the field order it was built
// with. Builders are mutable and return themselves, so calls can be
// chained; Build finishes a step or job, and WorkflowBuilder.Build finishes
// the document ready for emit.Print.
package core

import (
	"errors"
	"fmt"
	"strings"

	"github.com/preloopdev/loopt/sdk/go/om"
)

// ErrMissingStepID reports a step reference used without an explicit ID.
var ErrMissingStepID = errors.New("this step has no `id`, so its outputs/outcome/conclusion cannot be referenced")

// UndeclaredOutputError reports a reference to a job output the job did not
// declare.
type UndeclaredOutputError struct {
	Kind     string
	Name     string
	Declared []string
}

func (e *UndeclaredOutputError) Error() string {
	listed := "none"
	if len(e.Declared) > 0 {
		quoted := make([]string, len(e.Declared))
		for i, name := range e.Declared {
			quoted[i] = fmt.Sprintf("%q", name)
		}
		listed = strings.Join(quoted, ", ")
	}
	return fmt.Sprintf(
		"%s output %q is not declared; declared outputs: %s",
		e.Kind, e.Name, listed,
	)
}

// setField appends a field, or replaces an existing field's value in place,
// mirroring the last-write-wins behavior of a JSON object.
func setField(fields []om.Field, key string, value any) []om.Field {
	for i := range fields {
		if fields[i].Key == key {
			fields[i].Value = value
			return fields
		}
	}
	return append(fields, om.F(key, value))
}
