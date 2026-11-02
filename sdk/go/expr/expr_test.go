package expr

import (
	"errors"
	"math"
	"strings"
	"testing"
)

func must(t *testing.T) func(Expr, error) Expr {
	t.Helper()
	return func(e Expr, err error) Expr {
		t.Helper()
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		return e
	}
}

func TestLiteralRendering(t *testing.T) {
	tests := []struct {
		name  string
		value any
		want  string
		bare  string
	}{
		{name: "string", value: "hello", want: "${{ 'hello' }}", bare: "'hello'"},
		{name: "quote escaping", value: "it's", want: "${{ 'it''s' }}", bare: "'it''s'"},
		{name: "bool", value: true, want: "${{ true }}", bare: "true"},
		{name: "int", value: int64(-42), want: "${{ -42 }}", bare: "-42"},
		{name: "integer float", value: 5.0, want: "${{ 5 }}", bare: "5"},
		{name: "fraction", value: 1.5, want: "${{ 1.5 }}", bare: "1.5"},
		{name: "null", value: nil, want: "${{ null }}", bare: "null"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			expr := must(t)(Lit(test.value))
			if got := expr.String(); got != test.want {
				t.Fatalf("String() = %q, want %q", got, test.want)
			}
			if got := expr.Bare(); got != test.bare {
				t.Fatalf("Bare() = %q, want %q", got, test.bare)
			}
		})
	}
}

func TestOperatorPrecedenceParenthesization(t *testing.T) {
	ref := must(t)(Eq(Github().Field("ref"), "refs/heads/main"))
	event := must(t)(Eq(Github().Field("eventName"), "push"))

	or := must(t)(Or(ref, event))
	if got, want := or.Bare(), "github.ref == 'refs/heads/main' || github.event_name == 'push'"; got != want {
		t.Fatalf("Or() = %q, want %q", got, want)
	}
	if got := or.Precedence(); got != PrecedenceOr {
		t.Fatalf("Or precedence = %v", got)
	}

	// An or-chain nested under && must be parenthesized; an and-chain under
	// || binds tighter and must not be.
	and := must(t)(And(or, lit(t, true)))
	if got, want := and.Bare(), "(github.ref == 'refs/heads/main' || github.event_name == 'push') && true"; got != want {
		t.Fatalf("And(Or(...)) = %q, want %q", got, want)
	}
	flattened := must(t)(Or(must(t)(And(ref, event)), lit(t, false)))
	if got, want := flattened.Bare(), "github.ref == 'refs/heads/main' && github.event_name == 'push' || false"; got != want {
		t.Fatalf("Or(And(...)) = %q, want %q", got, want)
	}

	// Equality operands parenthesize at equal precedence.
	equality := must(t)(Eq(or, lit(t, true)))
	if got, want := equality.Bare(), "(github.ref == 'refs/heads/main' || github.event_name == 'push') == true"; got != want {
		t.Fatalf("Eq(Or(...)) = %q, want %q", got, want)
	}

	// Not wraps a comparison it binds tighter than.
	not := must(t)(Not(ref))
	if got, want := not.Bare(), "!(github.ref == 'refs/heads/main')"; got != want {
		t.Fatalf("Not(Eq(...)) = %q, want %q", got, want)
	}

	// A hand-built fragment can declare its precedence up front.
	raw := must(t)(WithPrecedence("github.a || github.b", PrecedenceOr))
	if got, want := must(t)(And(raw, lit(t, true))).Bare(), "(github.a || github.b) && true"; got != want {
		t.Fatalf("And(WithPrecedence(...)) = %q, want %q", got, want)
	}
}

func lit(t *testing.T, value any) Expr {
	t.Helper()
	return must(t)(Lit(value))
}

func TestMemberAccessAndStar(t *testing.T) {
	at := must(t)(At(Github(), "ref"))
	if got, want := at.Bare(), "github['ref']"; got != want {
		t.Fatalf("At() = %q, want %q", got, want)
	}
	star := must(t)(Star(Github().Field("event")))
	if got, want := star.Bare(), "github.event.*"; got != want {
		t.Fatalf("Star() = %q, want %q", got, want)
	}
	dynamicA := must(t)(At(Github(), must(t)(Or(lit(t, "a"), lit(t, "b")))))
	if got, want := dynamicA.Bare(), "github['a' || 'b']"; got != want {
		t.Fatalf("At(dynamic key) = %q, want %q", got, want)
	}
}

func TestContextsRenderAliasesAndTrackRoots(t *testing.T) {
	tests := []struct {
		name     string
		expr     Expr
		want     string
		contexts []string
	}{
		{
			name:     "secrets",
			expr:     Secrets().Field("TOKEN").Expr(),
			want:     "${{ secrets.TOKEN }}",
			contexts: []string{"secrets"},
		},
		{
			name:     "github alias",
			expr:     Github().Field("refName").Expr(),
			want:     "${{ github.ref_name }}",
			contexts: []string{"github"},
		},
		{
			name:     "nested event",
			expr:     Github().Field("event").Field("pull_request").Field("number").Expr(),
			want:     "${{ github.event.pull_request.number }}",
			contexts: []string{"github"},
		},
		{
			name:     "non-identifier property",
			expr:     Github().Field("branch.name").Expr(),
			want:     "${{ github['branch.name'] }}",
			contexts: []string{"github"},
		},
		{
			name:     "strategy alias with hyphen",
			expr:     Strategy().Field("failFast").Expr(),
			want:     "${{ strategy.fail-fast }}",
			contexts: []string{"strategy"},
		},
		{
			name:     "runner alias",
			expr:     Runner().Field("toolCache").Expr(),
			want:     "${{ runner.tool_cache }}",
			contexts: []string{"runner"},
		},
		{
			name:     "bare context",
			expr:     Env().Expr(),
			want:     "${{ env }}",
			contexts: []string{"env"},
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := test.expr.String(); got != test.want {
				t.Fatalf("String() = %q, want %q", got, test.want)
			}
			got := test.expr.Contexts()
			if strings.Join(got, ",") != strings.Join(test.contexts, ",") {
				t.Fatalf("Contexts() = %v, want %v", got, test.contexts)
			}
		})
	}
}

func TestContextDetectionIgnoresStringsAndFunctionNames(t *testing.T) {
	expr := must(t)(Raw("contains(github.ref, 'github.sha') && env.FOO != ''"))
	got := strings.Join(expr.Contexts(), ",")
	if want := "env,github"; got != want {
		t.Fatalf("Contexts() = %q, want %q", got, want)
	}
	if !expr.HasContext("GitHub") {
		t.Fatal("HasContext should be case-insensitive")
	}
}

func TestFunctions(t *testing.T) {
	hash := must(t)(HashFiles("**/go.sum", "**/go.mod"))
	if got, want := hash.String(), "${{ hashFiles('**/go.sum', '**/go.mod') }}"; got != want {
		t.Fatalf("HashFiles() = %q, want %q", got, want)
	}
	if !hash.HasContext("hashfiles") {
		t.Fatal("hashFiles must record the hashfiles context")
	}

	formatted := must(t)(Format("{0}-{1}", Github().Field("refName"), "suffix"))
	if got, want := formatted.String(), "${{ format('{0}-{1}', github.ref_name, 'suffix') }}"; got != want {
		t.Fatalf("Format() = %q, want %q", got, want)
	}

	joined := must(t)(Join(Github().Field("event").Field("commits"), "-"))
	if got, want := joined.String(), "${{ join(github.event.commits, '-') }}"; got != want {
		t.Fatalf("Join() = %q, want %q", got, want)
	}

	if got, want := Success().String(), "${{ success() }}"; got != want {
		t.Fatalf("Success() = %q, want %q", got, want)
	}
	if !Success().HasContext("success") {
		t.Fatal("success() must record its context-sensitive name")
	}
	if got, want := must(t)(Contains("abc", "b")).String(), "${{ contains('abc', 'b') }}"; got != want {
		t.Fatalf("Contains() = %q, want %q", got, want)
	}
}

func TestInvalidInputsReturnErrors(t *testing.T) {
	if _, err := Raw("   "); !errors.Is(err, ErrEmptyExpression) {
		t.Fatalf("Raw(empty) error = %v, want ErrEmptyExpression", err)
	}
	if _, err := Raw("${{ github.ref }}"); err == nil {
		t.Fatal("Raw with wrapper delimiter must fail")
	} else {
		var forbidden *ForbiddenDelimiterError
		if !errors.As(err, &forbidden) || forbidden.Delimiter != "${{" {
			t.Fatalf("error = %v, want ForbiddenDelimiterError(${{)", err)
		}
	}
	if _, err := Raw("'${{ literal }}'"); err != nil {
		t.Fatalf("delimiters inside a quoted literal must be allowed: %v", err)
	}
	if _, err := WithPrecedence("x", math.Inf(1)); !errors.Is(err, ErrNonFinitePrecedence) {
		t.Fatalf("WithPrecedence(inf) error = %v", err)
	}
	if _, err := Lit(math.NaN()); !errors.Is(err, ErrNonFiniteNumber) {
		t.Fatalf("Lit(NaN) error = %v", err)
	}
	if _, err := Lit(struct{}{}); err == nil {
		t.Fatal("Lit(unsupported type) must fail")
	}

	arityChecks := []func() error{
		func() error { _, err := And(lit(t, true)); return err },
		func() error { _, err := HashFiles(); return err },
		func() error { _, err := Case("a", "b"); return err },
		func() error { _, err := Join([]any{}, "x", "y"); return err },
	}
	for i, check := range arityChecks {
		var arity *ArityError
		if err := check(); !errors.As(err, &arity) {
			t.Fatalf("arity check %d: error = %v, want *ArityError", i, err)
		}
	}
}

func TestValueConvertsExpressionsRecursively(t *testing.T) {
	converted := Value(map[string]any{
		"if":   must(t)(Or(lit(t, "a"), lit(t, "b"))),
		"list": []any{Secrets().Field("TOKEN"), 1, nil},
	})
	if converted.(map[string]any)["if"] != "${{ 'a' || 'b' }}" {
		t.Fatalf("expression not converted: %#v", converted)
	}
	list := converted.(map[string]any)["list"].([]any)
	if list[0] != "${{ secrets.TOKEN }}" || list[1] != 1 || list[2] != nil {
		t.Fatalf("list not converted: %#v", list)
	}
	if got := Value(42); got != 42 {
		t.Fatalf("Value(42) = %#v", got)
	}
}
