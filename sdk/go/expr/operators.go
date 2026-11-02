package expr

// mergeContexts collects the lowercase context names referenced by exprs.
func mergeContexts(exprs []Expr) map[string]struct{} {
	contexts := make(map[string]struct{})
	for _, expr := range exprs {
		for name := range expr.contexts {
			contexts[name] = struct{}{}
		}
	}
	return contexts
}

// below parenthesizes expr when it binds less tightly than limit.
func below(expr Expr, limit float64) Expr {
	if expr.precedence >= limit {
		return expr
	}
	return composed("("+expr.source+")", expr.contexts, PrecedenceAtom)
}

// atMost parenthesizes expr when it binds no more tightly than limit.
func atMost(expr Expr, limit float64) Expr {
	if expr.precedence > limit {
		return expr
	}
	return composed("("+expr.source+")", expr.contexts, PrecedenceAtom)
}

// comparison builds a binary comparison whose operands are parenthesized at
// equal precedence.
func comparison(symbol string, limit float64, left, right any) (Expr, error) {
	leftExpr, err := Operand(left)
	if err != nil {
		return Expr{}, err
	}
	rightExpr, err := Operand(right)
	if err != nil {
		return Expr{}, err
	}
	leftExpr = atMost(leftExpr, limit)
	rightExpr = atMost(rightExpr, limit)
	contexts := mergeContexts([]Expr{leftExpr, rightExpr})
	source := leftExpr.source + " " + symbol + " " + rightExpr.source
	return composed(source, contexts, limit), nil
}

// Eq builds `left == right` at Equality precedence.
func Eq(left, right any) (Expr, error) {
	return comparison("==", PrecedenceEquality, left, right)
}

// Ne builds `left != right` at Equality precedence.
func Ne(left, right any) (Expr, error) {
	return comparison("!=", PrecedenceEquality, left, right)
}

// Lt builds `left < right` at Comparison precedence.
func Lt(left, right any) (Expr, error) {
	return comparison("<", PrecedenceComparison, left, right)
}

// Le builds `left <= right` at Comparison precedence.
func Le(left, right any) (Expr, error) {
	return comparison("<=", PrecedenceComparison, left, right)
}

// Gt builds `left > right` at Comparison precedence.
func Gt(left, right any) (Expr, error) {
	return comparison(">", PrecedenceComparison, left, right)
}

// Ge builds `left >= right` at Comparison precedence.
func Ge(left, right any) (Expr, error) {
	return comparison(">=", PrecedenceComparison, left, right)
}

// logical builds a short-circuit logical chain, flattening operands of equal
// precedence.
func logical(name, symbol string, limit float64, values []any) (Expr, error) {
	if len(values) < 2 {
		return Expr{}, &ArityError{Name: name, Requirement: "at least 2 arguments", Received: len(values)}
	}
	rendered := make([]Expr, len(values))
	for i, value := range values {
		expr, err := Operand(value)
		if err != nil {
			return Expr{}, err
		}
		rendered[i] = below(expr, limit)
	}
	sources := make([]string, len(rendered))
	for i, expr := range rendered {
		sources[i] = expr.source
	}
	return composed(join(sources, " "+symbol+" "), mergeContexts(rendered), limit), nil
}

// And builds `a && b && ...` at And precedence.
func And(values ...any) (Expr, error) {
	return logical("and", "&&", PrecedenceAnd, values)
}

// Or builds `a || b || ...` at Or precedence.
func Or(values ...any) (Expr, error) {
	return logical("or", "||", PrecedenceOr, values)
}

// Not builds `!value` at Not precedence.
func Not(value any) (Expr, error) {
	expr, err := Operand(value)
	if err != nil {
		return Expr{}, err
	}
	expr = below(expr, PrecedenceNot)
	return composed("!"+expr.source, expr.contexts, PrecedenceNot), nil
}

// At builds an index access `object[key]` at Member precedence. The key is
// rendered through Operand, so At(Github(), "ref") produces `github['ref']`.
func At(object, key any) (Expr, error) {
	base, err := Operand(object)
	if err != nil {
		return Expr{}, err
	}
	keyExpr, err := Operand(key)
	if err != nil {
		return Expr{}, err
	}
	base = below(base, PrecedenceMember)
	contexts := mergeContexts([]Expr{base, keyExpr})
	return composed(base.source+"["+keyExpr.source+"]", contexts, PrecedenceMember), nil
}

// Star builds an object-filter access `object.*` at Member precedence.
func Star(object any) (Expr, error) {
	base, err := Operand(object)
	if err != nil {
		return Expr{}, err
	}
	base = below(base, PrecedenceMember)
	return composed(base.source+".*", base.contexts, PrecedenceMember), nil
}

func join(values []string, separator string) string {
	if len(values) == 0 {
		return ""
	}
	joined := values[0]
	for _, value := range values[1:] {
		joined += separator + value
	}
	return joined
}
