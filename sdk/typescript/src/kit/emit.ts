import { LooptError } from "./errors.ts";
import { OrderedObject, stringify } from "./json.ts";
import type { DocValue } from "./json.ts";

/** Minimal view of Node's `process` needed by the default writer. */
declare const process: { stdout: { write(text: string): unknown } };

/** A value that renders itself to a JSON document before serialization. */
export interface Documentable {
  /** Returns the JSON document to emit. */
  toDocument(): DocValue;
}

/**
 * Serializes `source` to compact JSON and writes it as exactly one line.
 *
 * `source` may be a {@link Documentable}, in which case `toDocument()` supplies
 * the document, or the document value itself. The resolved value must be a
 * plain object or an {@link OrderedObject}.
 *
 * @throws {LooptError} `"emit-not-object"` when the resolved document is not an
 * object.
 */
export function emit(source: Documentable | DocValue, write?: (text: string) => void): void {
  const document: DocValue = isDocumentable(source) ? source.toDocument() : source;
  if (!isDocumentObject(document)) {
    throw new LooptError(
      "emit-not-object",
      "emit() expects a workflow document object, received " + typeof document,
    );
  }
  const writer: (text: string) => void =
    write ??
    ((text: string) => {
      process.stdout.write(text);
    });
  writer(stringify(document) + "\n");
}

/** Whether `value` exposes a callable `toDocument` member. */
function isDocumentable(value: unknown): value is Documentable {
  if (typeof value !== "object" || value === null || !("toDocument" in value)) {
    return false;
  }
  return typeof value.toDocument === "function";
}

/** Whether `value` is a plain object (or an {@link OrderedObject}) suitable as a document root. */
function isDocumentObject(
  value: unknown,
): value is { [key: string]: DocValue } | OrderedObject {
  if (value instanceof OrderedObject) {
    return true;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype: unknown = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
