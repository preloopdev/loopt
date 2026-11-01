import { Expr, LooptError, OrderedObject, RawNumber, isExpr, obj } from "../kit/index.ts";
import type { DocValue } from "../kit/index.ts";

/** The kinds of authoring references that get a placeholder token. */
export type TokenKind = "job" | "step";

/** Number assigned to the next generated token; shared by both token kinds. */
let nextTokenNumber = 1;

/**
 * Creates a fresh reference token for a job or step.
 *
 * A reference cannot know its final id when an expression is built, so
 * expressions embed this token where the id belongs; assembly later replaces
 * every token with the placed id. Tokens are valid dot-notation identifier
 * segments and are unique across the whole process.
 *
 * @param kind - Whether the token stands for a job or a step.
 */
export function newToken(kind: TokenKind): string {
  const number = nextTokenNumber;
  nextTokenNumber += 1;
  return "__loopt_" + kind + "_" + String(number) + "__";
}

/**
 * Matches a reference token anywhere inside a string. The pattern is global,
 * so callers iterating with `matchAll` or `exec` must reset `lastIndex`, or
 * use {@link tokensIn}.
 */
export const TOKEN_PATTERN: RegExp = /__loopt_(job|step)_[0-9]+__/g;

/** Anchored pattern matching exactly one complete reference token. */
const FULL_TOKEN_PATTERN = /^__loopt_(job|step)_[0-9]+__$/;

/**
 * Returns the kind stored in `token` when it is a complete reference token,
 * or `undefined` for any other string.
 */
export function tokenKind(token: string): TokenKind | undefined {
  if (typeof token !== "string") {
    return undefined;
  }
  const match = FULL_TOKEN_PATTERN.exec(token);
  return match === null ? undefined : (match[1] as TokenKind);
}

/**
 * Collects the distinct reference tokens in `text`, in order of first
 * appearance.
 */
export function tokensIn(text: string): string[] {
  if (typeof text !== "string" || !text.includes("__loopt_")) {
    return [];
  }
  const matches = text.match(TOKEN_PATTERN);
  if (matches === null) {
    return [];
  }
  const unique: string[] = [];
  const seen = new Set<string>();
  for (const token of matches) {
    if (!seen.has(token)) {
      seen.add(token);
      unique.push(token);
    }
  }
  return unique;
}

/**
 * Collects every reference token contained in `value`.
 *
 * Strings are scanned directly (including the `source` of an {@link Expr},
 * whose rendered `${{ ... }}` form carries the same text), arrays are walked
 * in order, and plain objects and {@link OrderedObject} entries are walked by
 * value. Class instances — `JobRef`, `StepRef`, `MatrixRef`, `RawNumber`,
 * `Map`, and anything else — are not descended into. The walk is cycle-safe
 * and ignores values it cannot traverse, so it never throws.
 */
export function collectTokens(value: unknown): Set<string> {
  const tokens = new Set<string>();
  const seen = new WeakSet<object>();

  const visit = (current: unknown): void => {
    if (typeof current === "string") {
      for (const token of tokensIn(current)) {
        tokens.add(token);
      }
      return;
    }
    if (typeof current !== "object" || current === null) {
      return;
    }
    if (isExpr(current)) {
      for (const token of tokensIn(current.source)) {
        tokens.add(token);
      }
      return;
    }
    if (seen.has(current)) {
      return;
    }
    seen.add(current);
    if (Array.isArray(current)) {
      for (const item of current) {
        visit(item);
      }
      return;
    }
    if (current instanceof OrderedObject) {
      for (const [, entryValue] of current.entries) {
        visit(entryValue);
      }
      return;
    }
    if (isPlainObject(current)) {
      for (const key of Object.keys(current)) {
        visit(current[key]);
      }
    }
  };

  visit(value);
  return tokens;
}

/**
 * Returns a deep copy of `value` in which every reference token inside a
 * string value — including the `source` of an {@link Expr} — is replaced by
 * `resolve(token)`. Mapping keys are left untouched.
 *
 * @throws {LooptError} `"unresolved-reference"` when `resolve` returns
 * `undefined` for a token present in the document.
 */
export function substituteTokens(
  value: DocValue,
  resolve: (token: string) => string | undefined,
): DocValue {
  return substituteValue(value, resolve);
}

/** Deep-copies one document value, substituting tokens in its strings. */
function substituteValue(
  value: DocValue,
  resolve: (token: string) => string | undefined,
): DocValue {
  if (value === null || typeof value === "boolean" || typeof value === "number") {
    return value;
  }
  if (typeof value === "string") {
    return substituteText(value, resolve);
  }
  if (value instanceof RawNumber) {
    return new RawNumber(value.literal);
  }
  if (isExpr(value)) {
    return new Expr(substituteText(value.source, resolve), {
      contexts: value.contexts,
      precedence: value.precedence,
    });
  }
  if (Array.isArray(value)) {
    return value.map((item) => substituteValue(item, resolve));
  }
  if (value instanceof OrderedObject) {
    return storedMapping(value.entries, resolve);
  }
  const entries: Array<[string, DocValue]> = [];
  for (const key of Object.keys(value)) {
    entries.push([key, value[key]]);
  }
  return storedMapping(entries, resolve);
}

/** Substitutes tokens in every string of `text`, rejecting unknown tokens. */
function substituteText(
  text: string,
  resolve: (token: string) => string | undefined,
): string {
  if (!text.includes("__loopt_")) {
    return text;
  }
  return text.replace(TOKEN_PATTERN, (token: string): string => {
    const replacement = resolve(token);
    if (replacement === undefined) {
      throw new LooptError(
        "unresolved-reference",
        "no job or step in this workflow matches reference token " + JSON.stringify(token),
      );
    }
    return replacement;
  });
}

/**
 * Copies mapping entries by value and rebuilds the mapping with the kit
 * helper, which preserves entry order and integer-like or `__proto__` keys.
 */
function storedMapping(
  entries: ReadonlyArray<readonly [string, DocValue]>,
  resolve: (token: string) => string | undefined,
): DocValue {
  const copied: Array<[string, DocValue]> = [];
  for (const [key, entry] of entries) {
    copied.push([key, substituteValue(entry, resolve)]);
  }
  return obj(copied);
}

/** Whether `value` is a plain object rather than a kit or user class instance. */
function isPlainObject(value: object): value is { [key: string]: DocValue } {
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
