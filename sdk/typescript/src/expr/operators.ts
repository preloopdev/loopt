import { Expr, LooptError, Precedence } from "../kit/index.ts";
import { operand } from "./literal.ts";

/** Collects the lowercase context names referenced by `expressions`. */
function mergeContexts(expressions: ReadonlyArray<Expr>): Set<string> {
  const contexts = new Set<string>();
  for (const expression of expressions) {
    for (const name of expression.contexts) {
      contexts.add(name.toLowerCase());
    }
  }
  return contexts;
}

/** Parenthesises `expression` when it binds less tightly than `precedence`. */
function below(expression: Expr, precedence: number): Expr {
  if (expression.precedence >= precedence) {
    return expression;
  }
  return new Expr("(" + expression.source + ")", {
    contexts: expression.contexts,
    precedence: Precedence.Atom,
  });
}

/** Parenthesises `expression` when it binds no more tightly than `precedence`. */
function atMost(expression: Expr, precedence: number): Expr {
  if (expression.precedence > precedence) {
    return expression;
  }
  return new Expr("(" + expression.source + ")", {
    contexts: expression.contexts,
    precedence: Precedence.Atom,
  });
}

/** Throws `"invalid-arity"` when `received` falls outside `min..=max`. */
function requireArity(name: string, received: number, min: number, max: number): void {
  if (received < min || received > max) {
    const expected = min === max ? String(min) : min + " to " + max;
    throw new LooptError(
      "invalid-arity",
      name + "() expects " + expected + " argument" + (max === 1 ? "" : "s") + "; received " + received,
    );
  }
}

/** Builds a binary comparison whose operands are parenthesised at equal precedence. */
function comparison(
  symbol: string,
  precedence: number,
  args: ReadonlyArray<unknown>,
): Expr<boolean> {
  const left = atMost(operand(args[0]), precedence);
  const right = atMost(operand(args[1]), precedence);
  return new Expr<boolean>(left.source + " " + symbol + " " + right.source, {
    contexts: mergeContexts([left, right]),
    precedence,
  });
}

/** Builds a short-circuit logical chain, flattening operands of equal precedence. */
function logical(
  name: string,
  symbol: string,
  precedence: number,
  values: ReadonlyArray<unknown>,
): Expr<boolean> {
  if (values.length < 2) {
    throw new LooptError(
      "invalid-arity",
      name + "() expects at least 2 arguments; received " + values.length,
    );
  }
  const rendered = values.map((value) => below(operand(value), precedence));
  return new Expr<boolean>(
    rendered.map((expression) => expression.source).join(" " + symbol + " "),
    {
      contexts: mergeContexts(rendered),
      precedence,
    },
  );
}

/**
 * Builds an equality comparison `left == right` at Equality precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function eq(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("eq", args.length, 2, 2);
  return comparison("==", Precedence.Equality, args);
}

/**
 * Builds an inequality comparison `left != right` at Equality precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function ne(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("ne", args.length, 2, 2);
  return comparison("!=", Precedence.Equality, args);
}

/**
 * Builds a less-than comparison `left < right` at Comparison precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function lt(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("lt", args.length, 2, 2);
  return comparison("<", Precedence.Comparison, args);
}

/**
 * Builds a less-than-or-equal comparison `left <= right` at Comparison
 * precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function lte(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("lte", args.length, 2, 2);
  return comparison("<=", Precedence.Comparison, args);
}

/**
 * Builds a greater-than comparison `left > right` at Comparison precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function gt(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("gt", args.length, 2, 2);
  return comparison(">", Precedence.Comparison, args);
}

/**
 * Builds a greater-than-or-equal comparison `left >= right` at Comparison
 * precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function gte(...args: [left: unknown, right: unknown]): Expr<boolean> {
  requireArity("gte", args.length, 2, 2);
  return comparison(">=", Precedence.Comparison, args);
}

/**
 * Builds a short-circuit conjunction `a && b && ...` at And precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when called with fewer than 2 arguments.
 */
export function and(...values: unknown[]): Expr<boolean> {
  return logical("and", "&&", Precedence.And, values);
}

/**
 * Builds a short-circuit disjunction `a || b || ...` at Or precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when called with fewer than 2 arguments.
 */
export function or(...values: unknown[]): Expr<boolean> {
  return logical("or", "||", Precedence.Or, values);
}

/**
 * Builds a logical negation `!value` at Not precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 1 argument.
 */
export function not(...args: [value: unknown]): Expr<boolean> {
  requireArity("not", args.length, 1, 1);
  const inner = below(operand(args[0]), Precedence.Not);
  return new Expr<boolean>("!" + inner.source, {
    contexts: mergeContexts([inner]),
    precedence: Precedence.Not,
  });
}

/**
 * Builds an index access `object[key]` at Member precedence. The key is
 * rendered through {@link operand}, so `at(github, "ref")` produces
 * `github['ref']`.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function at(...args: [object: unknown, key: unknown]): Expr {
  requireArity("at", args.length, 2, 2);
  const base = below(operand(args[0]), Precedence.Member);
  const key = operand(args[1]);
  return new Expr(base.source + "[" + key.source + "]", {
    contexts: mergeContexts([base, key]),
    precedence: Precedence.Member,
  });
}

/**
 * Builds an object-filter access `object.*` at Member precedence.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 1 argument.
 */
export function star(...args: [object: unknown]): Expr {
  requireArity("star", args.length, 1, 1);
  const base = below(operand(args[0]), Precedence.Member);
  return new Expr(base.source + ".*", {
    contexts: mergeContexts([base]),
    precedence: Precedence.Member,
  });
}
