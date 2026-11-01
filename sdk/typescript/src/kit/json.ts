import { LooptError } from "./errors.ts";
import { isExpr } from "./expr.ts";
import type { Expr } from "./expr.ts";

/** A scalar JSON value. */
export type Scalar = string | number | boolean;

/** A value that is either a plain literal or an expression producing that type. */
export type Val<T> = T | Expr<T>;

/** A JSON number literal preserved verbatim, used for integers outside the safe range. */
export class RawNumber {
  /** Exact decimal text of the number. */
  readonly literal: string;

  constructor(literal: string) {
    this.literal = literal;
  }
}

/**
 * An object whose entries keep exactly the order they were supplied in, which
 * plain JavaScript objects cannot do for integer-like keys (engines move those
 * ahead of every other key).
 */
export class OrderedObject {
  /** Entries in their authored order. */
  readonly entries: ReadonlyArray<readonly [string, DocValue]>;

  constructor(entries: ReadonlyArray<readonly [string, DocValue]>) {
    this.entries = entries;
  }
}

/** Any value the kit can serialize into a JSON document. */
export type DocValue =
  | null
  | boolean
  | number
  | string
  | RawNumber
  | Expr
  | DocValue[]
  | OrderedObject
  | { [key: string]: DocValue };

/** Matches the JSON number grammar from RFC 8259. */
const JSON_NUMBER = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;

/**
 * Validates `literal` as a JSON number and wraps it in a {@link RawNumber} so
 * it is emitted with exactly this text.
 *
 * @throws {LooptError} `"invalid-number"` when `literal` is not a JSON number.
 */
export function raw(literal: string): RawNumber {
  if (typeof literal !== "string") {
    throw new LooptError(
      "invalid-number",
      "raw() expects a valid JSON number literal, received a " + typeof literal,
    );
  }
  if (!JSON_NUMBER.test(literal)) {
    throw new LooptError(
      "invalid-number",
      "raw() expects a valid JSON number literal, received " + JSON.stringify(literal),
    );
  }
  return new RawNumber(literal);
}

/**
 * Builds an {@link OrderedObject} from `entries`, preserving their order
 * exactly.
 *
 * @throws {LooptError} `"duplicate-key"` when a key appears more than once.
 */
export function ordered(entries: Array<[string, DocValue]>): OrderedObject {
  assertUniqueKeys(entries, "ordered");
  return new OrderedObject(entries);
}

/**
 * Whether `key` is a canonical array-index string: `"0"` through
 * `"4294967294"`, with no sign, decimal point, exponent, or leading zero.
 */
export function isIntegerLikeKey(key: string): boolean {
  if (key.length === 0 || key.length > 10) {
    return false;
  }
  if (key.length > 1 && key[0] === "0") {
    return false;
  }
  for (let index = 0; index < key.length; index += 1) {
    const code = key.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return Number(key) <= 4294967294;
}

/**
 * Builds a JSON object from `entries`. A plain object is returned with keys in
 * insertion order, except when a key is integer-like or `"__proto__"`: those
 * would be reordered or treated specially by ordinary property assignment, so
 * an {@link OrderedObject} is returned instead.
 *
 * @throws {LooptError} `"duplicate-key"` when a key appears more than once.
 */
export function obj(entries: Array<[string, DocValue]>): { [key: string]: DocValue } | OrderedObject {
  assertUniqueKeys(entries, "obj");
  let needsOrdering = false;
  for (const [key] of entries) {
    if (key === "__proto__" || isIntegerLikeKey(key)) {
      needsOrdering = true;
      break;
    }
  }
  if (needsOrdering) {
    return new OrderedObject(entries);
  }
  const record: { [key: string]: DocValue } = {};
  for (const [key, value] of entries) {
    record[key] = value;
  }
  return record;
}

/**
 * Serializes `value` to compact JSON text, honouring {@link OrderedObject}
 * entry order and {@link RawNumber} exact text, and rendering {@link Expr}
 * instances as the JSON string of their `${{ ... }}` form.
 *
 * @throws {LooptError} `"non-finite-number"` for `NaN` and infinities.
 * @throws {LooptError} `"unsupported-value"` for `undefined`, functions,
 * bigints, and symbols.
 */
export function stringify(value: DocValue): string {
  return serialize(value);
}

/** Joins `items` with newlines, without a trailing newline. */
export function lines(...items: string[]): string {
  return items.join("\n");
}

/** Throws when `entries` repeats a key. */
function assertUniqueKeys(entries: ReadonlyArray<readonly [string, DocValue]>, helper: string): void {
  const seen = new Set<string>();
  for (const [key] of entries) {
    if (seen.has(key)) {
      throw new LooptError(
        "duplicate-key",
        helper + "() received the key " + JSON.stringify(key) + " more than once",
      );
    }
    seen.add(key);
  }
}

/** Recursive workhorse behind {@link stringify}; accepts `unknown` so invalid values can be reported. */
function serialize(value: unknown): string {
  if (value === null) {
    return "null";
  }
  if (typeof value === "string") {
    return jsonString(value);
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new LooptError("non-finite-number", "cannot serialize a non-finite number as JSON");
    }
    return String(value);
  }
  if (typeof value !== "object") {
    throw new LooptError(
      "unsupported-value",
      "cannot serialize a value of type " + typeof value + " as JSON",
    );
  }
  if (value instanceof RawNumber) {
    return value.literal;
  }
  if (value instanceof OrderedObject) {
    return (
      "{" +
      value.entries
        .map(([key, entry]) => jsonString(key) + ":" + serialize(entry))
        .join(",") +
      "}"
    );
  }
  if (Array.isArray(value)) {
    return "[" + value.map((entry) => serialize(entry)).join(",") + "]";
  }
  if (isExpr(value)) {
    return jsonString(value.toString());
  }
  return (
    "{" +
    Object.entries(value)
      .map(([key, entry]) => jsonString(key) + ":" + serialize(entry))
      .join(",") +
    "}"
  );
}

/**
 * Escapes `value` as a double-quoted JSON string literal. Control characters,
 * quotes, and backslashes are escaped, and lone surrogates are emitted as
 * `\uXXXX` so the output is always well-formed JSON text.
 */
function jsonString(value: string): string {
  let out = '"';
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index] ?? "";
    const code = value.charCodeAt(index);
    switch (char) {
      case '"':
        out += '\\"';
        break;
      case "\\":
        out += "\\\\";
        break;
      case "\n":
        out += "\\n";
        break;
      case "\r":
        out += "\\r";
        break;
      case "\t":
        out += "\\t";
        break;
      case "\b":
        out += "\\b";
        break;
      case "\f":
        out += "\\f";
        break;
      default:
        if (code < 0x20) {
          out += "\\u" + code.toString(16).padStart(4, "0");
        } else if (code >= 0xd800 && code <= 0xdbff) {
          const next = index + 1 < value.length ? value.charCodeAt(index + 1) : 0;
          if (next >= 0xdc00 && next <= 0xdfff) {
            out += char + (value[index + 1] ?? "");
            index += 1;
          } else {
            out += "\\u" + code.toString(16).padStart(4, "0");
          }
        } else if (code >= 0xdc00 && code <= 0xdfff) {
          out += "\\u" + code.toString(16).padStart(4, "0");
        } else {
          out += char;
        }
    }
  }
  return out + '"';
}
