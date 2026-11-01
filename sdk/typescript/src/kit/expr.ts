import { LooptError } from "./errors.ts";

/**
 * Operator precedence levels for GitHub Actions expressions, ordered from the
 * loosest binding (`Or`) to the tightest (`Atom`).
 */
export const Precedence: {
  readonly Or: 1;
  readonly And: 2;
  readonly Equality: 3;
  readonly Comparison: 4;
  readonly Not: 5;
  readonly Member: 6;
  readonly Atom: 7;
} = {
  Or: 1,
  And: 2,
  Equality: 3,
  Comparison: 4,
  Not: 5,
  Member: 6,
  Atom: 7,
};

/** Options accepted by the {@link Expr} constructor. */
export interface ExprOptions {
  /**
   * Extra context names to record alongside the ones detected in `source`.
   * Names are stored lowercase.
   */
  readonly contexts?: Iterable<string>;
  /** Operator precedence of the expression; defaults to {@link Precedence.Atom}. */
  readonly precedence?: number;
}

/**
 * A GitHub Actions expression: the bare expression text plus its `${{ ... }}`
 * wrapper, the top-level contexts it references, and its operator precedence.
 *
 * Instances serialize to their wrapped form when interpolated into a JSON
 * document, so they can be used anywhere a plain value is accepted.
 */
export class Expr<T = unknown> {
  /** Bare expression text, e.g. `github.ref == 'refs/heads/main'`. */
  readonly source: string;

  /** Lowercase names of the top-level contexts referenced by the expression. */
  readonly contexts: ReadonlySet<string>;

  /** Operator precedence of the expression, used when composing larger expressions. */
  readonly precedence: number;

  /** Phantom marker carrying `T` at compile time; never present at runtime. */
  declare readonly __type?: T;

  constructor(source: string, options: ExprOptions = {}) {
    if (typeof source !== "string") {
      throw new LooptError("invalid-expression", "expression source must be a string");
    }
    if (source.trim().length === 0) {
      throw new LooptError("invalid-expression", "expression source must not be empty or whitespace");
    }
    const forbidden = findForbiddenDelimiter(source);
    if (forbidden !== null) {
      throw new LooptError(
        "invalid-expression",
        "expression source must not contain the wrapper delimiter " +
          JSON.stringify(forbidden) +
          " outside a single-quoted string literal",
      );
    }
    const precedence = options.precedence ?? Precedence.Atom;
    if (!Number.isFinite(precedence)) {
      throw new LooptError("invalid-expression", "expression precedence must be a finite number");
    }
    const contexts = detectContexts(source);
    if (options.contexts !== undefined) {
      for (const name of options.contexts) {
        contexts.add(name.toLowerCase());
      }
    }
    this.source = source;
    this.contexts = contexts;
    this.precedence = precedence;
  }

  /** Renders the expression with its `${{ ... }}` wrapper, for use in a YAML value. */
  toString(): string {
    return "${{ " + this.source + " }}";
  }

  /** Returns the bare expression text without the `${{ ... }}` wrapper. */
  bare(): string {
    return this.source;
  }
}

/** Wraps `source` in an {@link Expr} with default options. */
export function rawExpr<T = unknown>(source: string): Expr<T> {
  return new Expr<T>(source);
}

/** Type guard for {@link Expr} instances. */
export function isExpr(value: unknown): value is Expr {
  return value instanceof Expr;
}

/**
 * Returns the first `${{` or `}}` found outside a single-quoted string
 * literal, or `null` when the source is clean. GitHub expression strings
 * escape a single quote by doubling it.
 */
function findForbiddenDelimiter(source: string): string | null {
  let inString = false;
  for (let index = 0; index < source.length; index += 1) {
    const char = source[index];
    if (inString) {
      if (char === "'") {
        if (source[index + 1] === "'") {
          index += 1;
        } else {
          inString = false;
        }
      }
      continue;
    }
    if (char === "'") {
      inString = true;
      continue;
    }
    if (source.startsWith("${{", index)) {
      return "${{";
    }
    if (source.startsWith("}}", index)) {
      return "}}";
    }
  }
  return null;
}

/**
 * Collects the lowercase top-level context names referenced by `source`.
 *
 * A top-level context is an identifier followed (after optional whitespace) by
 * `.` or `[`, that is itself not the tail of a member access. Identifiers
 * inside single-quoted string literals are ignored, as are function names
 * (which are followed by an opening parenthesis).
 */
function detectContexts(source: string): Set<string> {
  const contexts = new Set<string>();
  let inString = false;
  let index = 0;
  while (index < source.length) {
    const char = source[index];
    if (inString) {
      if (char === "'") {
        if (source[index + 1] === "'") {
          index += 2;
        } else {
          inString = false;
          index += 1;
        }
      } else {
        index += 1;
      }
      continue;
    }
    if (char === "'") {
      inString = true;
      index += 1;
      continue;
    }
    if (!/[A-Za-z_]/.test(char)) {
      index += 1;
      continue;
    }
    let end = index + 1;
    while (end < source.length && /[A-Za-z0-9_-]/.test(source[end] ?? "")) {
      end += 1;
    }
    const name = source.slice(index, end);
    let lookahead = end;
    while (lookahead < source.length && /[ \t\n\r]/.test(source[lookahead] ?? "")) {
      lookahead += 1;
    }
    const next = lookahead < source.length ? (source[lookahead] ?? "") : "";
    const previous = previousSignificant(source, index);
    if (
      (next === "." || next === "[") &&
      previous !== "." &&
      previous !== "[" &&
      previous !== "'"
    ) {
      contexts.add(name.toLowerCase());
    }
    index = end;
  }
  return contexts;
}

/** Returns the closest non-whitespace character before `index`, or `""`. */
function previousSignificant(source: string, index: number): string {
  let cursor = index - 1;
  while (cursor >= 0 && /[ \t\n\r]/.test(source[cursor] ?? "")) {
    cursor -= 1;
  }
  return cursor >= 0 ? (source[cursor] ?? "") : "";
}
