import { Expr, LooptError, Precedence } from "../kit/index.ts";
import { operand } from "./literal.ts";

/**
 * Builds a call expression at Atom precedence. Arguments are rendered through
 * {@link operand} and joined with `", "`; `extraContexts` records the
 * context-sensitive functions the call depends on.
 */
function call<T>(
  name: string,
  args: ReadonlyArray<unknown>,
  extraContexts: ReadonlyArray<string> = [],
): Expr<T> {
  const rendered = args.map((argument) => operand(argument));
  const contexts = new Set<string>();
  for (const contextName of extraContexts) {
    contexts.add(contextName.toLowerCase());
  }
  for (const expression of rendered) {
    for (const contextName of expression.contexts) {
      contexts.add(contextName.toLowerCase());
    }
  }
  return new Expr<T>(name + "(" + rendered.map((expression) => expression.source).join(", ") + ")", {
    contexts,
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

/**
 * Builds `contains(search, item)`, which tests whether `search` contains
 * `item` (case-insensitive for strings).
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function contains(...args: [search: unknown, item: unknown]): Expr<boolean> {
  requireArity("contains", args.length, 2, 2);
  return call<boolean>("contains", args);
}

/**
 * Builds `startsWith(value, prefix)`.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function startsWith(...args: [value: unknown, prefix: unknown]): Expr<boolean> {
  requireArity("startsWith", args.length, 2, 2);
  return call<boolean>("startsWith", args);
}

/**
 * Builds `endsWith(value, suffix)`.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 2 arguments.
 */
export function endsWith(...args: [value: unknown, suffix: unknown]): Expr<boolean> {
  requireArity("endsWith", args.length, 2, 2);
  return call<boolean>("endsWith", args);
}

/**
 * Builds `format(template, ...values)`, substituting each `{N}` placeholder in
 * `template` with the corresponding value.
 *
 * @throws {LooptError} `"invalid-arity"` when called with no arguments.
 */
export function format(...args: [template: unknown, ...values: unknown[]]): Expr<string> {
  if (args.length < 1) {
    throw new LooptError(
      "invalid-arity",
      "format() expects at least 1 argument; received " + args.length,
    );
  }
  return call<string>("format", args);
}

/**
 * Builds `join(array, separator?)`, which concatenates the array elements
 * with the optional separator (default `","`).
 *
 * @throws {LooptError} `"invalid-arity"` when called with more than 2 arguments.
 */
export function join(...args: [array: unknown, separator?: unknown]): Expr<string> {
  requireArity("join", args.length, 1, 2);
  const values: unknown[] = [args[0]];
  if (args.length === 2 && args[1] !== undefined) {
    values.push(args[1]);
  }
  return call<string>("join", values);
}

/**
 * Builds `toJSON(value)`, which renders a value as a pretty-printed JSON
 * string.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 1 argument.
 */
export function toJSON(...args: [value: unknown]): Expr<string> {
  requireArity("toJSON", args.length, 1, 1);
  return call<string>("toJSON", args);
}

/**
 * Builds `fromJSON(value)`, which parses a JSON string.
 *
 * @throws {LooptError} `"invalid-arity"` when not called with exactly 1 argument.
 */
export function fromJSON<T = unknown>(...args: [value: unknown]): Expr<T> {
  requireArity("fromJSON", args.length, 1, 1);
  return call<T>("fromJSON", args);
}

/**
 * Builds `hashFiles(...patterns)`, which hashes the files matching the given
 * glob patterns. The result records the `hashfiles` context-sensitive
 * function name.
 *
 * @throws {LooptError} `"invalid-arity"` when called with fewer than 1 or more
 * than 255 patterns.
 * @throws {LooptError} `"unsupported-value"` when a pattern is not a string.
 */
export function hashFiles(...patterns: string[]): Expr<string> {
  requireArity("hashFiles", patterns.length, 1, 255);
  for (const pattern of patterns) {
    if (typeof pattern !== "string") {
      throw new LooptError("unsupported-value", "hashFiles() expects string patterns");
    }
  }
  return call<string>("hashFiles", patterns, ["hashfiles"]);
}

/**
 * Builds `case(value, predicate, result, ...)`, GitHub's lazy conditional
 * chain. The argument count must be odd so that every predicate has a result
 * and the chain ends with a default.
 *
 * @throws {LooptError} `"invalid-arity"` when called with an even number of
 * arguments or fewer than 3.
 */
export function caseOf(...args: unknown[]): Expr<unknown> {
  if (args.length < 3 || args.length % 2 === 0) {
    throw new LooptError(
      "invalid-arity",
      "caseOf() expects an odd number of at least 3 arguments; received " + args.length,
    );
  }
  return call<unknown>("case", args);
}

/** Builds a status function call that records its own context name. */
function status(name: string, args: ReadonlyArray<unknown>): Expr<boolean> {
  if (args.length !== 0) {
    throw new LooptError(
      "invalid-arity",
      name + "() expects no arguments; received " + args.length,
    );
  }
  return call<boolean>(name, [], [name]);
}

/**
 * Builds `success()`, true when no previous step has failed. Records the
 * `success` context-sensitive function name.
 *
 * @throws {LooptError} `"invalid-arity"` when called with arguments.
 */
export function success(...args: []): Expr<boolean> {
  return status("success", args);
}

/**
 * Builds `failure()`, true when any previous step has failed. Records the
 * `failure` context-sensitive function name.
 *
 * @throws {LooptError} `"invalid-arity"` when called with arguments.
 */
export function failure(...args: []): Expr<boolean> {
  return status("failure", args);
}

/**
 * Builds `always()`, which always evaluates to true. Records the `always`
 * context-sensitive function name.
 *
 * @throws {LooptError} `"invalid-arity"` when called with arguments.
 */
export function always(...args: []): Expr<boolean> {
  return status("always", args);
}

/**
 * Builds `cancelled()`, true when the workflow was cancelled. Records the
 * `cancelled` context-sensitive function name.
 *
 * @throws {LooptError} `"invalid-arity"` when called with arguments.
 */
export function cancelled(...args: []): Expr<boolean> {
  return status("cancelled", args);
}
