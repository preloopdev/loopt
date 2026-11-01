import { LooptError } from "./errors.ts";
import type { DocValue } from "./json.ts";

/** Mixin shape for authoring helpers that accept arbitrary extra properties. */
export type Extra = { extra?: Record<string, DocValue> };

/**
 * Appends the entries of `extra` after `entries`, preserving `extra`'s own key
 * order. The input array is not mutated.
 *
 * @throws {LooptError} `"extra-conflict"` when an `extra` key already exists in
 * `entries` (or repeats within `extra`).
 */
export function applyExtra(
  entries: Array<[string, DocValue]>,
  extra: Record<string, DocValue> | undefined,
): Array<[string, DocValue]> {
  if (extra === undefined) {
    return entries;
  }
  const existing = new Set<string>();
  for (const [key] of entries) {
    existing.add(key);
  }
  const appended: Array<[string, DocValue]> = [];
  for (const [key, value] of Object.entries(extra)) {
    if (existing.has(key)) {
      throw new LooptError(
        "extra-conflict",
        "extra key " + JSON.stringify(key) + " conflicts with an existing key",
      );
    }
    existing.add(key);
    appended.push([key, value]);
  }
  if (appended.length === 0) {
    return entries;
  }
  return [...entries, ...appended];
}
