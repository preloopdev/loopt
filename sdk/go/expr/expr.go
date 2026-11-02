// Package expr builds GitHub Actions `${{ ... }}` expressions: literals,
// operators, built-in functions, and dynamically navigable contexts.
//
// Expressions are immutable values. Implements fmt.Stringer with the
// wrapped form (`${{ source }}`) and falls back to the bare source text via
// Bare. Every constructor that can fail returns an error rather than
// panicking, so composed expressions report the same invalid input the
// TypeScript and Rust SDKs reject.
package expr

import (
	"errors"
	"fmt"
	"math"
	"sort"
	"strings"
)

// Operator precedence levels for GitHub Actions expressions, ordered from
// the loosest binding (Or) to the tightest (Atom).
const (
	PrecedenceOr         = 1.0
	PrecedenceAnd        = 2.0
	PrecedenceEquality   = 3.0
	PrecedenceComparison = 4.0
	PrecedenceNot        = 5.0
	PrecedenceMember     = 6.0
	PrecedenceAtom       = 7.0
)

// ErrEmptyExpression reports an expression source that is empty or only
// whitespace.
var ErrEmptyExpression = errors.New("expression source must not be empty or whitespace")

// ErrNonFinitePrecedence reports an explicit precedence that is NaN or
// infinite.
var ErrNonFinitePrecedence = errors.New("expression precedence must be a finite number")

// ErrNonFiniteNumber reports a NaN or infinite number literal.
var ErrNonFiniteNumber = errors.New("cannot render a non-finite number as an expression literal")

// ForbiddenDelimiterError reports a `${{` or `}}` found outside a
// single-quoted string literal.
type ForbiddenDelimiterError struct {
	Delimiter string
}

func (e *ForbiddenDelimiterError) Error() string {
	return fmt.Sprintf(
		"expression source must not contain the wrapper delimiter %q outside a single-quoted string literal",
		e.Delimiter,
	)
}

// ArityError reports a variadic function called with an unsupported number
// of arguments.
type ArityError struct {
	Name        string
	Requirement string
	Received    int
}

func (e *ArityError) Error() string {
	return fmt.Sprintf("%s() expects %s; received %d", e.Name, e.Requirement, e.Received)
}

// Expr is a GitHub Actions expression: its bare source text, the top-level
// contexts it references, and its operator precedence.
type Expr struct {
	source     string
	contexts   map[string]struct{}
	precedence float64
}

// Raw wraps source in an expression at Atom precedence, recording any
// top-level contexts it references.
func Raw(source string) (Expr, error) {
	return newExpr(source, nil, PrecedenceAtom)
}

// WithPrecedence wraps source in an expression at an explicit operator
// precedence, so hand-built fragments can compose with And/Or/Not without
// an extra, redundant parenthesization.
func WithPrecedence(source string, precedence float64) (Expr, error) {
	return newExpr(source, nil, precedence)
}

func newExpr(source string, extraContexts []string, precedence float64) (Expr, error) {
	if strings.TrimSpace(source) == "" {
		return Expr{}, ErrEmptyExpression
	}
	if delimiter, found := findForbiddenDelimiter(source); found {
		return Expr{}, &ForbiddenDelimiterError{Delimiter: delimiter}
	}
	if isNonFinite(precedence) {
		return Expr{}, ErrNonFinitePrecedence
	}
	contexts := detectContexts(source)
	for _, name := range extraContexts {
		contexts[strings.ToLower(name)] = struct{}{}
	}
	return Expr{source: source, contexts: contexts, precedence: precedence}, nil
}

// rootContext builds a bare context expression named name. Its contexts set
// contains name's lowercase form even though a bare name has no following
// `.` or `[` for detectContexts to pick up on its own.
func rootContext(name string) (Expr, error) {
	return newExpr(name, []string{name}, PrecedenceAtom)
}

// composed builds an Expr from already-validated parts, skipping the
// validation that newExpr performs. Operators and functions assemble their
// source from previously validated sub-expressions and fixed glue text
// (`.`, `[`, `]`, `(`, `)`, `, `, operator symbols, quoted literals), which
// can never form a forbidden delimiter or an empty source.
func composed(source string, contexts map[string]struct{}, precedence float64) Expr {
	return Expr{source: source, contexts: contexts, precedence: precedence}
}

// Source returns the bare expression text, e.g.
// `github.ref == 'refs/heads/main'`.
func (e Expr) Source() string {
	return e.source
}

// Bare returns the bare expression text without the `${{ ... }}` wrapper.
func (e Expr) Bare() string {
	return e.source
}

// Precedence returns the expression's operator precedence, used when
// composing larger expressions.
func (e Expr) Precedence() float64 {
	return e.precedence
}

// Contexts returns the lowercase names of the top-level contexts referenced
// by the expression, sorted.
func (e Expr) Contexts() []string {
	names := make([]string, 0, len(e.contexts))
	for name := range e.contexts {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

// HasContext reports whether the expression references the named
// top-level context (case-insensitively).
func (e Expr) HasContext(name string) bool {
	_, ok := e.contexts[strings.ToLower(name)]
	return ok
}

// String renders the wrapped form, `${{ source }}`.
func (e Expr) String() string {
	return "${{ " + e.source + " }}"
}

// Value converts an Expr or Context to its wrapped `${{ ... }}` string
// form so it can be stored in a workflow value. Slices and
// map[string]any are converted recursively; any other value passes
// through unchanged.
func Value(value any) any {
	switch v := value.(type) {
	case Expr:
		return v.String()
	case *Expr:
		if v == nil {
			return nil
		}
		return v.String()
	case Context:
		return v.String()
	case *Context:
		if v == nil {
			return nil
		}
		return v.String()
	case []any:
		converted := make([]any, len(v))
		for i, item := range v {
			converted[i] = Value(item)
		}
		return converted
	case map[string]any:
		converted := make(map[string]any, len(v))
		for key, item := range v {
			converted[key] = Value(item)
		}
		return converted
	default:
		return value
	}
}

// findForbiddenDelimiter returns the first `${{` or `}}` found outside a
// single-quoted string literal. GitHub expression strings escape a single
// quote by doubling it.
func findForbiddenDelimiter(source string) (string, bool) {
	chars := []rune(source)
	inString := false
	for index := 0; index < len(chars); {
		ch := chars[index]
		if inString {
			if ch == '\'' {
				if index+1 < len(chars) && chars[index+1] == '\'' {
					index++
				} else {
					inString = false
				}
			}
			index++
			continue
		}
		if ch == '\'' {
			inString = true
			index++
			continue
		}
		if matchesAt(chars, index, "${{") {
			return "${{", true
		}
		if matchesAt(chars, index, "}}") {
			return "}}", true
		}
		index++
	}
	return "", false
}

func matchesAt(chars []rune, index int, pattern string) bool {
	runes := []rune(pattern)
	if index+len(runes) > len(chars) {
		return false
	}
	for i, r := range runes {
		if chars[index+i] != r {
			return false
		}
	}
	return true
}

func isWhitespace(c rune) bool {
	return c == ' ' || c == '\t' || c == '\n' || c == '\r'
}

func isNameChar(c rune) bool {
	return isASCIIAlphanumeric(c) || c == '_' || c == '-'
}

func isASCIIAlphanumeric(c rune) bool {
	return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')
}

func isASCIIAlphabetic(c rune) bool {
	return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
}

// previousSignificant returns the closest non-whitespace character before
// index, or -1 when there is none.
func previousSignificant(chars []rune, index int) rune {
	for cursor := index - 1; cursor >= 0; cursor-- {
		if !isWhitespace(chars[cursor]) {
			return chars[cursor]
		}
	}
	return -1
}

// detectContexts collects the lowercase top-level context names referenced
// by source. A top-level context is an identifier followed (after optional
// whitespace) by `.` or `[`, that is itself not the tail of a member
// access. Identifiers inside single-quoted string literals are ignored, as
// are function names (followed by an opening parenthesis).
func detectContexts(source string) map[string]struct{} {
	chars := []rune(source)
	contexts := make(map[string]struct{})
	inString := false
	index := 0
	for index < len(chars) {
		ch := chars[index]
		if inString {
			if ch == '\'' {
				if index+1 < len(chars) && chars[index+1] == '\'' {
					index += 2
				} else {
					inString = false
					index++
				}
			} else {
				index++
			}
			continue
		}
		if ch == '\'' {
			inString = true
			index++
			continue
		}
		if !(isASCIIAlphabetic(ch) || ch == '_') {
			index++
			continue
		}
		start := index
		end := index + 1
		for end < len(chars) && isNameChar(chars[end]) {
			end++
		}
		lookahead := end
		for lookahead < len(chars) && isWhitespace(chars[lookahead]) {
			lookahead++
		}
		var next rune = -1
		if lookahead < len(chars) {
			next = chars[lookahead]
		}
		previous := previousSignificant(chars, start)
		if (next == '.' || next == '[') && previous != '.' && previous != '[' && previous != '\'' {
			contexts[strings.ToLower(string(chars[start:end]))] = struct{}{}
		}
		index = end
	}
	return contexts
}

func isNonFinite(value float64) bool {
	return math.IsNaN(value) || math.IsInf(value, 0)
}
