package core

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/preloopdev/loopt/sdk/go/expr"
	"github.com/preloopdev/loopt/sdk/go/om"
)

func mustExpr(t *testing.T) func(expr.Expr, error) expr.Expr {
	t.Helper()
	return func(value expr.Expr, err error) expr.Expr {
		t.Helper()
		if err != nil {
			t.Fatalf("unexpected expression error: %v", err)
		}
		return value
	}
}

func marshal(t *testing.T, value any) string {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("Marshal: %v", err)
	}
	return string(encoded)
}

func TestStepFieldOrderAndValues(t *testing.T) {
	condition := mustExpr(t)(expr.Eq(expr.Github().Field("refName"), "main"))
	step := Run("go test ./...").
		ID("test").
		Name("Test").
		If(condition).
		With("working-directory", "sub").
		Env("CGO_ENABLED", "0").
		Shell("bash").
		WorkingDirectory("sub").
		ContinueOnError(true).
		TimeoutMinutes(10).
		Build()

	want := `{"id":"test","if":"${{ github.ref_name == 'main' }}","name":"Test","run":"go test ./...","with":{"working-directory":"sub"},"shell":"bash","working-directory":"sub","env":{"CGO_ENABLED":"0"},"continue-on-error":true,"timeout-minutes":10}`
	if got := marshal(t, step.Value()); got != want {
		t.Fatalf("step JSON =\n%s\nwant\n%s", got, want)
	}
}

func TestUsesStep(t *testing.T) {
	step := Uses("actions/checkout@v4").ID("checkout").Build()
	want := `{"id":"checkout","uses":"actions/checkout@v4"}`
	if got := marshal(t, step.Value()); got != want {
		t.Fatalf("step JSON = %s, want %s", got, want)
	}
}

func TestIfAcceptsExpressionsContextsAndRawStrings(t *testing.T) {
	step := Run("true").If(expr.Github().Field("refName")).Build()
	if got, want := marshal(t, step.Value()), `{"if":"${{ github.ref_name }}","run":"true"}`; got != want {
		t.Fatalf("context if = %s, want %s", got, want)
	}

	job := NewJob().If("github.event_name == 'push'").RunsOn("ubuntu-latest").Step(Run("true").Build()).Build()
	if got := marshal(t, job.Value()); !strings.Contains(got, `"if":"github.event_name == 'push'"`) {
		t.Fatalf("raw if = %s", got)
	}
}

func TestStepRefOutputsRequireAnID(t *testing.T) {
	step := Run("echo hi").ID("build").Build()
	output := mustExpr(t)(step.Output("artifact"))
	if got, want := output.String(), "${{ steps.build.outputs.artifact }}"; got != want {
		t.Fatalf("Output() = %q, want %q", got, want)
	}
	if got, want := mustExpr(t)(step.Outcome()).String(), "${{ steps.build.outcome }}"; got != want {
		t.Fatalf("Outcome() = %q, want %q", got, want)
	}
	if got, want := mustExpr(t)(step.Conclusion()).String(), "${{ steps.build.conclusion }}"; got != want {
		t.Fatalf("Conclusion() = %q, want %q", got, want)
	}

	anonymous := Run("echo hi").Build()
	if _, err := anonymous.Output("artifact"); !errors.Is(err, ErrMissingStepID) {
		t.Fatalf("anonymous Output error = %v, want ErrMissingStepID", err)
	}
	if _, err := anonymous.Outcome(); !errors.Is(err, ErrMissingStepID) {
		t.Fatalf("anonymous Outcome error = %v, want ErrMissingStepID", err)
	}
	if _, err := anonymous.Conclusion(); !errors.Is(err, ErrMissingStepID) {
		t.Fatalf("anonymous Conclusion error = %v, want ErrMissingStepID", err)
	}
	if _, ok := anonymous.ID(); ok {
		t.Fatal("anonymous step reports an ID")
	}
}

func TestJobRefValidatesDeclaredOutputs(t *testing.T) {
	producer := Run("make artifact").ID("build").Build()
	job := NewJob().
		RunsOn("ubuntu-latest").
		Output("artifact", mustExpr(t)(producer.Output("artifact"))).
		Step(producer).
		Build()

	workflow := NewWorkflow().Name("CI")
	ref := workflow.AddJob("build", job)

	output := mustExpr(t)(ref.Output("artifact"))
	if got, want := output.String(), "${{ needs.build.outputs.artifact }}"; got != want {
		t.Fatalf("Output() = %q, want %q", got, want)
	}
	if got, want := ref.Result().String(), "${{ needs.build.result }}"; got != want {
		t.Fatalf("Result() = %q, want %q", got, want)
	}

	_, err := ref.Output("missing")
	var undeclared *UndeclaredOutputError
	if !errors.As(err, &undeclared) {
		t.Fatalf("Output(missing) error = %v, want *UndeclaredOutputError", err)
	}
	if undeclared.Name != "missing" || len(undeclared.Declared) != 1 || undeclared.Declared[0] != "artifact" {
		t.Fatalf("undeclared error = %#v", undeclared)
	}
	if message := err.Error(); !strings.Contains(message, `declared outputs: "artifact"`) {
		t.Fatalf("error message = %q", message)
	}
}

func TestCallJobOutputsAreUnrestricted(t *testing.T) {
	workflow := NewWorkflow()
	ref := workflow.AddJob("reuse", NewCallJob("./.github/workflows/reusable.yml").
		With("target", "prod").
		Secrets("token", expr.Secrets().Field("DEPLOY_TOKEN")).
		Build())

	output := mustExpr(t)(ref.Output("anything"))
	if got, want := output.String(), "${{ needs.reuse.outputs.anything }}"; got != want {
		t.Fatalf("Output() = %q, want %q", got, want)
	}
}

func TestJobNeedsAndFieldOrder(t *testing.T) {
	first := NewJob().RunsOn("ubuntu-latest").Step(Run("true").Build()).Build()
	second := NewJob().RunsOn("ubuntu-latest").Step(Run("true").Build()).Build()
	third := NewJob().RunsOn("ubuntu-latest").Step(Run("true").Build()).Build()

	workflow := NewWorkflow().Name("CI")
	firstRef := workflow.AddJob("first", first)
	secondRef := workflow.AddJob("second", second)
	workflow.AddJob("third", third)

	job := NewJob().
		Name("Fan-in").
		Needs(firstRef, secondRef).
		RunsOn("ubuntu-latest").
		TimeoutMinutes(5).
		Env("CI", "true").
		Output("result", "ok").
		Step(Run("echo done").Build()).
		Build()

	want := `{"name":"Fan-in","needs":["first","second"],"runs-on":"ubuntu-latest","timeout-minutes":5,"env":{"CI":"true"},"outputs":{"result":"ok"},"steps":[{"run":"echo done"}]}`
	if got := marshal(t, job.Value()); got != want {
		t.Fatalf("job JSON =\n%s\nwant\n%s", got, want)
	}

	single := NewJob().Needs(firstRef).RunsOn("ubuntu-latest").Step(Run("true").Build()).Build()
	if got := marshal(t, single.Value()); !strings.Contains(got, `"needs":"first"`) {
		t.Fatalf("single needs must render as a string: %s", got)
	}
}

func TestWorkflowFieldOrderAndJobOrder(t *testing.T) {
	workflow := NewWorkflow().
		Name("CI").
		On(om.New(om.F("push", om.New(om.F("branches", []any{"main"}))), om.F("pull_request", om.New()))).
		Permissions(om.New(om.F("contents", "read"))).
		Env("GLOBAL", "1").
		Concurrency("ci-${{ github.ref }}").
		Defaults(om.New(om.F("run", om.New(om.F("shell", "bash")))))

	build := NewJob().RunsOn("ubuntu-latest").Step(Run("go build").Build()).Build()
	workflow.AddJob("build", build)
	test := NewJob().RunsOn("ubuntu-latest").Step(Run("go test").Build()).Build()
	workflow.AddJob("test", test)

	encoded := marshal(t, workflow.Build())
	order := []string{`"name"`, `"on"`, `"permissions"`, `"env"`, `"concurrency"`, `"defaults"`, `"jobs"`}
	previous := -1
	for _, key := range order {
		position := strings.Index(encoded, key)
		if position <= previous {
			t.Fatalf("key %s out of order in %s", key, encoded)
		}
		previous = position
	}
	if buildPosition, testPosition := strings.Index(encoded, `"build"`), strings.Index(encoded, `"test"`); buildPosition > testPosition {
		t.Fatalf("jobs reordered: %s", encoded)
	}
}

func TestAddJobReplacesInPlace(t *testing.T) {
	workflow := NewWorkflow()
	workflow.AddJob("build", NewJob().RunsOn("ubuntu-latest").Step(Run("old").Build()).Build())
	workflow.AddJob("test", NewJob().RunsOn("ubuntu-latest").Step(Run("test").Build()).Build())
	workflow.AddJob("build", NewJob().RunsOn("ubuntu-latest").Step(Run("new").Build()).Build())

	encoded := marshal(t, workflow.Build())
	if strings.Count(encoded, `"build"`) != 1 {
		t.Fatalf("duplicate job id: %s", encoded)
	}
	if strings.Index(encoded, `"build"`) > strings.Index(encoded, `"test"`) {
		t.Fatalf("replacing a job moved it: %s", encoded)
	}
	if !strings.Contains(encoded, "new") || strings.Contains(encoded, "old") {
		t.Fatalf("job value was not replaced: %s", encoded)
	}
}

func TestExpressionValuesAreWrappedInWorkflowFields(t *testing.T) {
	condition := expr.Success()
	workflow := NewWorkflow().
		On(expr.Github().Field("eventName")).
		Env("REF", expr.Github().Field("ref"))
	job := NewJob().
		If(condition).
		RunsOn(expr.Runner().Field("os")).
		Step(Run("true").Build()).
		Build()
	workflow.AddJob("build", job)

	encoded := marshal(t, workflow.Build())
	for _, want := range []string{
		`"on":"${{ github.event_name }}"`,
		`"REF":"${{ github.ref }}"`,
		`"if":"${{ success() }}"`,
		`"runs-on":"${{ runner.os }}"`,
	} {
		if !strings.Contains(encoded, want) {
			t.Fatalf("missing %s in %s", want, encoded)
		}
	}
}
