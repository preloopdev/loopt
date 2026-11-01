import { Expr, LooptError, Precedence, isExpr } from "../kit/index.ts";

/**
 * Renders `value` as a GitHub Actions literal expression at
 * {@link Precedence.Atom} precedence.
 *
 * Strings are single-quoted with embedded single quotes doubled, finite
 * numbers use their JavaScript text, booleans render as `true`/`false`, and
 * `null` renders as `null`.
 *
 * @throws {LooptError} `"non-finite-number"` when `value` is `NaN` or an
 * infinity.
 */
export function lit(value: string | number | boolean | null): Expr {
  if (typeof value === "string") {
    return new Expr("'" + value.replace(/'/g, "''") + "'", {
      precedence: Precedence.Atom,
    });
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new LooptError(
        "non-finite-number",
        "cannot render a non-finite number as an expression literal",
      );
    }
    return new Expr(String(value), { precedence: Precedence.Atom });
  }
  if (typeof value === "boolean") {
    return new Expr(value ? "true" : "false", { precedence: Precedence.Atom });
  }
  return new Expr("null", { precedence: Precedence.Atom });
}

/**
 * Coerces `value` into an expression. Expressions pass through unchanged and
 * primitive literals are wrapped with {@link lit}; every other value is
 * rejected.
 *
 * @throws {LooptError} `"unsupported-value"` when `value` is not an expression
 * and not a string, number, boolean, or null.
 */
export function operand(value: unknown): Expr {
  if (isExpr(value)) {
    return value;
  }
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return lit(value);
  }
  throw new LooptError(
    "unsupported-value",
    "cannot convert a value of type " + typeof value + " into an expression",
  );
}
