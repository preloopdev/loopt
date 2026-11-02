package expr

import (
	"fmt"
	"math"
	"strconv"
	"strings"
)

// quote renders value as a GitHub Actions single-quoted string literal,
// doubling any embedded single quote.
func quote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", "''") + "'"
}

// Lit renders a scalar value as a GitHub Actions literal expression at Atom
// precedence. Strings are single-quoted with embedded quotes doubled,
// finite numbers use their shortest round-trip decimal text, booleans
// render as true/false, and nil renders as null.
func Lit(value any) (Expr, error) {
	var source string
	switch v := value.(type) {
	case nil:
		source = "null"
	case string:
		source = quote(v)
	case bool:
		if v {
			source = "true"
		} else {
			source = "false"
		}
	case int:
		source = strconv.FormatInt(int64(v), 10)
	case int8:
		source = strconv.FormatInt(int64(v), 10)
	case int16:
		source = strconv.FormatInt(int64(v), 10)
	case int32:
		source = strconv.FormatInt(int64(v), 10)
	case int64:
		source = strconv.FormatInt(v, 10)
	case uint:
		source = strconv.FormatUint(uint64(v), 10)
	case uint8:
		source = strconv.FormatUint(uint64(v), 10)
	case uint16:
		source = strconv.FormatUint(uint64(v), 10)
	case uint32:
		source = strconv.FormatUint(uint64(v), 10)
	case uint64:
		source = strconv.FormatUint(v, 10)
	case float32:
		rendered, err := floatLiteral(float64(v), 32)
		if err != nil {
			return Expr{}, err
		}
		source = rendered
	case float64:
		rendered, err := floatLiteral(v, 64)
		if err != nil {
			return Expr{}, err
		}
		source = rendered
	default:
		return Expr{}, fmt.Errorf("lit: unsupported literal type %T", value)
	}
	return composed(source, map[string]struct{}{}, PrecedenceAtom), nil
}

// floatLiteral renders a finite float the way JavaScript's Number#toString
// would: the shortest decimal text that round-trips. Go's 'g' format
// already omits a trailing `.0` for integer-valued floats, so 5.0 renders
// as "5".
func floatLiteral(value float64, bitSize int) (string, error) {
	if math.IsNaN(value) || math.IsInf(value, 0) {
		return "", ErrNonFiniteNumber
	}
	return strconv.FormatFloat(value, 'g', -1, bitSize), nil
}

// Operand converts a value accepted where an operator or function expects
// an operand into an Expr: expressions and contexts pass through unchanged,
// and everything else is coerced through Lit.
func Operand(value any) (Expr, error) {
	switch v := value.(type) {
	case Expr:
		return v, nil
	case *Expr:
		if v == nil {
			return Expr{}, fmt.Errorf("operand: nil *Expr")
		}
		return *v, nil
	case Context:
		return v.Expr(), nil
	case *Context:
		if v == nil {
			return Expr{}, fmt.Errorf("operand: nil *Context")
		}
		return v.Expr(), nil
	default:
		return Lit(value)
	}
}
