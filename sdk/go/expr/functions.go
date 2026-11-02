package expr

import "strings"

// call builds a function-call expression at Atom precedence. Arguments are
// rendered through Operand and joined with ", "; extraContexts records the
// context-sensitive functions the call depends on (e.g. hashFiles's
// "hashfiles", or a status function's own name).
func call(name string, extraContexts []string, args ...any) (Expr, error) {
	rendered := make([]Expr, len(args))
	for i, arg := range args {
		expr, err := Operand(arg)
		if err != nil {
			return Expr{}, err
		}
		rendered[i] = expr
	}
	contexts := make(map[string]struct{}, len(extraContexts))
	for _, extra := range extraContexts {
		contexts[strings.ToLower(extra)] = struct{}{}
	}
	sources := make([]string, len(rendered))
	for i, expr := range rendered {
		sources[i] = expr.source
		for context := range expr.contexts {
			contexts[context] = struct{}{}
		}
	}
	return composed(name+"("+join(sources, ", ")+")", contexts, PrecedenceAtom), nil
}

// Contains builds `contains(search, item)`, which tests whether search
// contains item (case-insensitive for strings).
func Contains(search, item any) (Expr, error) {
	return call("contains", nil, search, item)
}

// StartsWith builds `startsWith(value, prefix)`.
func StartsWith(value, prefix any) (Expr, error) {
	return call("startsWith", nil, value, prefix)
}

// EndsWith builds `endsWith(value, suffix)`.
func EndsWith(value, suffix any) (Expr, error) {
	return call("endsWith", nil, value, suffix)
}

// Format builds `format(template, ...values)`, substituting each `{N}`
// placeholder in template with the corresponding value.
func Format(template any, values ...any) (Expr, error) {
	args := make([]any, 0, len(values)+1)
	args = append(args, template)
	args = append(args, values...)
	return call("format", nil, args...)
}

// Join builds `join(array, separator?)`, which concatenates the array
// elements with the optional separator (default ",").
func Join(array any, separator ...any) (Expr, error) {
	if len(separator) > 1 {
		return Expr{}, &ArityError{Name: "join", Requirement: "at most 1 separator", Received: len(separator)}
	}
	args := make([]any, 0, 2)
	args = append(args, array)
	args = append(args, separator...)
	return call("join", nil, args...)
}

// ToJSON builds `toJSON(value)`, which renders a value as a pretty-printed
// JSON string.
func ToJSON(value any) (Expr, error) {
	return call("toJSON", nil, value)
}

// FromJSON builds `fromJSON(value)`, which parses a JSON string.
func FromJSON(value any) (Expr, error) {
	return call("fromJSON", nil, value)
}

// HashFiles builds `hashFiles(...patterns)`, which hashes the files
// matching the given glob patterns. The result records the "hashfiles"
// context-sensitive function name.
func HashFiles(patterns ...string) (Expr, error) {
	if len(patterns) == 0 || len(patterns) > 255 {
		return Expr{}, &ArityError{Name: "hashFiles", Requirement: "1 to 255 patterns", Received: len(patterns)}
	}
	args := make([]any, len(patterns))
	for i, pattern := range patterns {
		args[i] = pattern
	}
	return call("hashFiles", []string{"hashfiles"}, args...)
}

// Case builds `case(value, predicate, result, ...)`, GitHub's lazy
// conditional chain. The argument count must be odd so every predicate has a
// result and the chain ends with a default.
func Case(args ...any) (Expr, error) {
	if len(args) < 3 || len(args)%2 == 0 {
		return Expr{}, &ArityError{
			Name:        "case",
			Requirement: "an odd number of at least 3 arguments",
			Received:    len(args),
		}
	}
	return call("case", nil, args...)
}

// status builds a status function call that records its own context name.
func status(name string) Expr {
	return composed(name+"()", map[string]struct{}{name: {}}, PrecedenceAtom)
}

// Success builds `success()`, true when no previous step has failed.
func Success() Expr {
	return status("success")
}

// Failure builds `failure()`, true when any previous step has failed.
func Failure() Expr {
	return status("failure")
}

// Always builds `always()`, which always evaluates to true.
func Always() Expr {
	return status("always")
}

// Cancelled builds `cancelled()`, true when the workflow was cancelled.
func Cancelled() Expr {
	return status("cancelled")
}
