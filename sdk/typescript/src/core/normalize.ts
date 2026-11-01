import { applyExtra } from "../kit/extra.ts";
import { LooptError } from "../kit/errors.ts";
import { isExpr } from "../kit/expr.ts";
import { OrderedObject, RawNumber, obj } from "../kit/json.ts";
import type { DocValue } from "../kit/json.ts";
import { ACTION_ALIASES, WORKFLOW_ALIASES } from "../generated/aliases.ts";
import { ACTION_SCHEMA, WORKFLOW_ROOT, WORKFLOW_SCHEMA } from "../generated/model.ts";
import type { ContextSpec, Property, Schema } from "../generated/model.ts";

/** Name of the schema a definition belongs to. */
export type SchemaName = "workflow" | "action";

/** A `[key, value]` pair for a normalized mapping entry. */
type Entry = [string, DocValue];

/** Read-only view of authored mapping entries. */
type EntryList = ReadonlyArray<readonly [string, DocValue]>;

/** A generated schema paired with its camelCase alias table. */
interface SchemaTables {
  readonly schema: Schema;
  readonly aliases: Record<string, Record<string, string>>;
}

/** The two generated schema/alias pairs the normalizer understands. */
const TABLES: Record<SchemaName, SchemaTables> = {
  workflow: { schema: WORKFLOW_SCHEMA, aliases: WORKFLOW_ALIASES },
  action: { schema: ACTION_SCHEMA, aliases: ACTION_ALIASES },
};

/** Builtin type references that are resolved without a schema definition. */
type BuiltinName = "string" | "number" | "boolean" | "null" | "sequence" | "mapping" | "any";

/** Kinds a flattened `one-of` candidate can have. */
type CandidateKind =
  | "null"
  | "boolean"
  | "number"
  | "string"
  | "allowed-values"
  | "sequence"
  | "mapping"
  | "any";

/** Structural view of a `sequence` definition. */
interface SequenceDefinition {
  readonly kind: "sequence";
  readonly item: string;
}

/** Structural view of a `mapping` definition. */
interface MappingDefinition {
  readonly kind: "mapping";
  readonly properties: Record<string, Property>;
  readonly looseValue?: string;
}

/** Structural view of a `string` definition. */
interface StringDefinition {
  readonly kind: "string";
  readonly constant?: string;
}

/** Structural view of a `one-of` definition. */
interface OneOfDefinition {
  readonly kind: "one-of";
  readonly members: string[];
}

/**
 * The discriminant-bearing union inside the generated `Definition` type, which
 * is that union intersected with shared metadata. The intersection is
 * distributed and split per kind so that switches over it are exhaustive.
 */
type DefinitionShape =
  | ({ description?: string; contexts?: ContextSpec[] } & { kind: "null" })
  | ({ description?: string; contexts?: ContextSpec[] } & { kind: "boolean" })
  | ({ description?: string; contexts?: ContextSpec[] } & { kind: "number" })
  | ({
      description?: string;
      contexts?: ContextSpec[];
    } & { kind: "string"; constant?: string; requireNonEmpty?: true })
  | ({
      description?: string;
      contexts?: ContextSpec[];
    } & { kind: "allowed-values"; values: string[] })
  | ({
      description?: string;
      contexts?: ContextSpec[];
    } & { kind: "sequence"; item: string })
  | ({
      description?: string;
      contexts?: ContextSpec[];
    } & {
      kind: "mapping";
      properties: Record<string, Property>;
      looseKey?: string;
      looseValue?: string;
    })
  | ({
      description?: string;
      contexts?: ContextSpec[];
    } & { kind: "one-of"; members: string[] });

/** Result of resolving one authored key against a mapping definition. */
interface KeyResolution {
  /** Exact YAML key the authored key maps to. */
  readonly exact: string;
  /** Type reference used to normalize the child value. */
  readonly type: string;
}

/**
 * Normalizes an authored value against a named schema definition.
 *
 * Properties may be written with camelCase names from the generated types or
 * with their exact YAML names; the result is a pure document whose mappings use
 * exact YAML names, built with {@link obj} so integer-like and `__proto__` keys
 * keep their order. No {@link Expr} survives: expressions render as
 * `${{ ... }}`, except under a definition whose name ends in `-if`, where they
 * render as bare expression text.
 *
 * @param schema Which generated schema to use.
 * @param definition Exact schema definition name, e.g. `workflow-root-strict`.
 * @param value Authored value; may contain `Expr`, `RawNumber`, `OrderedObject`,
 * arrays, and plain objects.
 * @param path JSON-pointer-like base path used in error messages.
 * @throws {LooptError} `"unknown-definition"`, `"type-mismatch"`,
 * `"unknown-key"`, `"duplicate-key"`, `"missing-required"`,
 * `"extra-conflict"`, or `"no-matching-shape"`.
 */
export function normalize(
  schema: SchemaName,
  definition: string,
  value: DocValue,
  path = "",
): DocValue {
  return normalizeDefinition(TABLES[schema], definition, value, path);
}

/**
 * Normalizes an authored workflow root against the strict workflow schema.
 *
 * @param root Workflow root with camelCase or exact YAML keys, including a
 * `jobs` mapping keyed by job id.
 * @returns The normalized document with exact YAML keys and no expressions.
 */
export function normalizeWorkflow(root: { [key: string]: DocValue }): DocValue {
  return normalize("workflow", WORKFLOW_ROOT, root, "");
}

/**
 * Looks up a definition as its discriminant-bearing shape, or `undefined` when
 * `name` is not a schema definition.
 */
function definitionOf(tables: SchemaTables, name: string): DefinitionShape | undefined {
  if (!Object.hasOwn(tables.schema.definitions, name)) {
    return undefined;
  }
  // The generated definition type intersects shared metadata with a union of
  // kinds; the shape below mirrors its members exactly so switches over `kind`
  // stay exhaustive.
  return tables.schema.definitions[name] as unknown as DefinitionShape;
}

/** Returns the camelCase alias table for `name`, or an empty table when absent. */
function aliasesOf(tables: SchemaTables, name: string): Record<string, string | undefined> {
  if (!Object.hasOwn(tables.aliases, name)) {
    return {};
  }
  return tables.aliases[name];
}

/** Whether `name` is a builtin type reference rather than a schema definition. */
function isBuiltinName(name: string): name is BuiltinName {
  return (
    name === "string" ||
    name === "number" ||
    name === "boolean" ||
    name === "null" ||
    name === "sequence" ||
    name === "mapping" ||
    name === "any"
  );
}

/** Normalizes `value` against one named definition or builtin type reference. */
function normalizeDefinition(
  tables: SchemaTables,
  name: string,
  value: DocValue,
  path: string,
): DocValue {
  const bareExpression = name.endsWith("-if");
  const definition = definitionOf(tables, name);
  if (definition !== undefined) {
    if (isExpr(value)) {
      return bareExpression ? value.bare() : value.toString();
    }
    return normalizeByDefinition(tables, name, definition, value, path);
  }
  if (isBuiltinName(name)) {
    if (isExpr(value)) {
      return bareExpression ? value.bare() : value.toString();
    }
    return normalizeBuiltin(name, value, path);
  }
  throw new LooptError(
    "unknown-definition",
    "unknown schema definition " + JSON.stringify(name) + at(path),
  );
}

/** Dispatches on the definition kind. */
function normalizeByDefinition(
  tables: SchemaTables,
  name: string,
  definition: DefinitionShape,
  value: DocValue,
  path: string,
): DocValue {
  switch (definition.kind) {
    case "null":
      if (value === null) {
        return null;
      }
      throw mismatch("null", value, path);
    case "boolean":
      if (typeof value === "boolean") {
        return value;
      }
      throw mismatch("a boolean", value, path);
    case "number":
      if (typeof value === "number" || value instanceof RawNumber) {
        return value;
      }
      throw mismatch("a number", value, path);
    case "string":
      return normalizeString(definition, value, path);
    case "allowed-values":
      if (typeof value === "string" && definition.values.includes(value)) {
        return value;
      }
      throw mismatch(
        "one of " + definition.values.map((allowed) => JSON.stringify(allowed)).join(", "),
        value,
        path,
      );
    case "sequence":
      return normalizeSequence(tables, definition, value, path);
    case "mapping":
      return normalizeMapping(tables, name, definition, value, path);
    case "one-of":
      return normalizeOneOf(tables, name, definition, value, path);
    default:
      throw new LooptError(
        "type-mismatch",
        "unsupported definition kind for " + name + at(path),
      );
  }
}

/** Normalizes a scalar against a `string` definition. */
function normalizeString(
  definition: StringDefinition,
  value: DocValue,
  path: string,
): DocValue {
  if (definition.constant !== undefined) {
    if (value === definition.constant) {
      return value;
    }
    throw mismatch("the constant " + JSON.stringify(definition.constant), value, path);
  }
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return value;
  }
  throw mismatch("a string", value, path);
}

/** Normalizes an array against a `sequence` definition. */
function normalizeSequence(
  tables: SchemaTables,
  definition: SequenceDefinition,
  value: DocValue,
  path: string,
): DocValue {
  if (!Array.isArray(value)) {
    throw mismatch("an array", value, path);
  }
  return value.map((item, index) =>
    normalizeDefinition(tables, definition.item, item, path + "/" + String(index)),
  );
}

/** Normalizes an object against a `mapping` definition. */
function normalizeMapping(
  tables: SchemaTables,
  name: string,
  definition: MappingDefinition,
  value: DocValue,
  path: string,
): DocValue {
  const entries = mappingEntries(value);
  if (entries === undefined) {
    throw mismatch("an object", value, path);
  }
  const aliases = aliasesOf(tables, name);
  const properties = definition.properties;
  const output: Entry[] = [];
  const used = new Map<string, string>();
  let extra: Record<string, DocValue> | undefined;
  let hasExtra = false;
  for (const [key, child] of entries) {
    if (key === "extra") {
      if (hasExtra) {
        throw new LooptError(
          "duplicate-key",
          "duplicate key " + JSON.stringify("extra") + " for " + name + at(path),
        );
      }
      hasExtra = true;
      extra = extraRecord(child, path);
      continue;
    }
    const resolution = resolveKey(properties, aliases, definition.looseValue, key);
    if (resolution === undefined) {
      throw new LooptError(
        "unknown-key",
        "unknown key " + JSON.stringify(key) + " for " + name + at(path),
      );
    }
    const previous = used.get(resolution.exact);
    if (previous !== undefined) {
      throw new LooptError(
        "duplicate-key",
        "duplicate key " +
          JSON.stringify(resolution.exact) +
          " supplied as " +
          JSON.stringify(previous) +
          " and " +
          JSON.stringify(key) +
          " for " +
          name +
          at(path),
      );
    }
    used.set(resolution.exact, key);
    output.push([
      resolution.exact,
      normalizeDefinition(tables, resolution.type, child, path + "/" + resolution.exact),
    ]);
  }
  for (const [propertyName, property] of Object.entries(properties)) {
    if (property.required === true && !used.has(propertyName)) {
      throw new LooptError(
        "missing-required",
        "missing required key " + JSON.stringify(propertyName) + " for " + name + at(path),
      );
    }
  }
  return obj(appendExtras(output, extra, path));
}

/** Normalizes `value` against a `one-of` definition by picking a fitting candidate. */
function normalizeOneOf(
  tables: SchemaTables,
  name: string,
  definition: OneOfDefinition,
  value: DocValue,
  path: string,
): DocValue {
  const leaves = flattenOneOf(tables, definition);
  const candidate = selectCandidate(tables, leaves, value, name, path);
  return normalizeDefinition(tables, candidate, value, path);
}

/** Normalizes an unconstrained builtin type reference. */
function normalizeBuiltin(name: BuiltinName, value: DocValue, path: string): DocValue {
  switch (name) {
    case "string":
      if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
        return value;
      }
      throw mismatch("a string", value, path);
    case "number":
      if (typeof value === "number" || value instanceof RawNumber) {
        return value;
      }
      throw mismatch("a number", value, path);
    case "boolean":
      if (typeof value === "boolean") {
        return value;
      }
      throw mismatch("a boolean", value, path);
    case "null":
      if (value === null) {
        return null;
      }
      throw mismatch("null", value, path);
    case "sequence":
    case "mapping":
    case "any":
      return copyDocument(value);
  }
}

/** Flattens nested `one-of` members into leaf type references, guarding cycles. */
function flattenOneOf(tables: SchemaTables, definition: OneOfDefinition): string[] {
  const leaves: string[] = [];
  const active = new Set<string>();
  const visit = (member: string): void => {
    const shape = definitionOf(tables, member);
    if (shape !== undefined && shape.kind === "one-of") {
      if (active.has(member)) {
        return;
      }
      active.add(member);
      for (const inner of shape.members) {
        visit(inner);
      }
      active.delete(member);
      return;
    }
    leaves.push(member);
  };
  for (const member of definition.members) {
    visit(member);
  }
  return leaves;
}

/** Returns the candidate kind of a leaf type reference. */
function candidateKind(tables: SchemaTables, leaf: string): CandidateKind | undefined {
  if (isBuiltinName(leaf)) {
    return leaf;
  }
  const shape = definitionOf(tables, leaf);
  if (shape === undefined || shape.kind === "one-of") {
    return undefined;
  }
  return shape.kind;
}

/**
 * Picks the first candidate leaf that can accept `value`. For an object that no
 * mapping candidate accepts outright, returns the candidate with the fewest
 * problems (unresolved keys plus absent required properties) so normalization
 * raises its specific error instead of a generic shape error; ties keep the
 * first candidate in schema order.
 */
function selectCandidate(
  tables: SchemaTables,
  leaves: string[],
  value: DocValue,
  owner: string,
  path: string,
): string {
  if (value === null) {
    const leaf = findLeafOfKind(tables, leaves, "null");
    if (leaf !== undefined) {
      return leaf;
    }
  } else if (typeof value === "string") {
    const leaf = selectStringLeaf(tables, leaves, value);
    if (leaf !== undefined) {
      return leaf;
    }
  } else if (typeof value === "number" || value instanceof RawNumber) {
    const leaf = findLeafOfKind(tables, leaves, "number");
    if (leaf !== undefined) {
      return leaf;
    }
  } else if (typeof value === "boolean") {
    const leaf = findLeafOfKind(tables, leaves, "boolean");
    if (leaf !== undefined) {
      return leaf;
    }
  } else if (Array.isArray(value)) {
    const leaf = findLeafOfKind(tables, leaves, "sequence");
    if (leaf !== undefined) {
      return leaf;
    }
  } else {
    const entries = mappingEntries(value);
    if (entries !== undefined) {
      let bestName: string | undefined;
      let bestProblems = Number.POSITIVE_INFINITY;
      for (const leaf of leaves) {
        const kind = candidateKind(tables, leaf);
        if (kind !== "mapping") {
          continue;
        }
        const shape = definitionOf(tables, leaf);
        if (shape === undefined) {
          return leaf;
        }
        if (shape.kind !== "mapping") {
          continue;
        }
        const problems = mappingProblemCount(tables, leaf, shape, entries);
        if (problems === 0) {
          return leaf;
        }
        if (problems < bestProblems) {
          bestName = leaf;
          bestProblems = problems;
        }
      }
      if (bestName !== undefined) {
        return bestName;
      }
    }
  }
  const anyLeaf = findLeafOfKind(tables, leaves, "any");
  if (anyLeaf !== undefined) {
    return anyLeaf;
  }
  throw new LooptError(
    "no-matching-shape",
    "value does not match any shape of " +
      owner +
      "; candidates: " +
      leaves.join(", ") +
      at(path),
  );
}

/** Finds the first leaf whose kind is `kind`. */
function findLeafOfKind(
  tables: SchemaTables,
  leaves: string[],
  kind: CandidateKind,
): string | undefined {
  return leaves.find((leaf) => candidateKind(tables, leaf) === kind);
}

/**
 * Picks the first string-compatible leaf, preferring an `allowed-values`
 * candidate that lists the value over a plain string candidate.
 */
function selectStringLeaf(
  tables: SchemaTables,
  leaves: string[],
  value: string,
): string | undefined {
  let fallback: string | undefined;
  for (const leaf of leaves) {
    const kind = candidateKind(tables, leaf);
    if (kind !== "string" && kind !== "allowed-values") {
      continue;
    }
    const shape = definitionOf(tables, leaf);
    if (shape === undefined) {
      if (fallback === undefined) {
        fallback = leaf;
      }
      continue;
    }
    if (shape.kind === "allowed-values") {
      if (shape.values.includes(value)) {
        return leaf;
      }
      continue;
    }
    if (
      shape.kind === "string" &&
      (shape.constant === undefined || shape.constant === value)
    ) {
      if (fallback === undefined) {
        fallback = leaf;
      }
    }
  }
  return fallback;
}

/**
 * Counts why a mapping candidate cannot accept `entries`: keys that do not
 * resolve to a property or loose key, plus required properties that are absent.
 */
function mappingProblemCount(
  tables: SchemaTables,
  name: string,
  definition: MappingDefinition,
  entries: EntryList,
): number {
  const aliases = aliasesOf(tables, name);
  const used = new Set<string>();
  let problems = 0;
  for (const [key] of entries) {
    if (key === "extra") {
      continue;
    }
    const resolution = resolveKey(definition.properties, aliases, definition.looseValue, key);
    if (resolution === undefined) {
      problems += 1;
      continue;
    }
    used.add(resolution.exact);
  }
  for (const [propertyName, property] of Object.entries(definition.properties)) {
    if (property.required === true && !used.has(propertyName)) {
      problems += 1;
    }
  }
  return problems;
}

/**
 * Resolves one authored mapping key: camelCase alias first, then an exact
 * property name, then the mapping's loose key. Returns `undefined` when the key
 * is unknown.
 */
function resolveKey(
  properties: Record<string, Property>,
  aliases: Record<string, string | undefined>,
  looseValue: string | undefined,
  key: string,
): KeyResolution | undefined {
  const alias = Object.hasOwn(aliases, key) ? aliases[key] : undefined;
  if (alias !== undefined && Object.hasOwn(properties, alias)) {
    return { exact: alias, type: properties[alias].type };
  }
  if (Object.hasOwn(properties, key)) {
    return { exact: key, type: properties[key].type };
  }
  if (looseValue !== undefined) {
    return { exact: key, type: looseValue };
  }
  return undefined;
}

/**
 * Returns the mapping entries of `value`, reading {@link OrderedObject} in its
 * authored order and a plain object in its own key order.
 */
function mappingEntries(value: DocValue): EntryList | undefined {
  if (value instanceof OrderedObject) {
    return value.entries;
  }
  if (isPlainObject(value)) {
    return Object.entries(value);
  }
  return undefined;
}

/** Whether `value` is a plain object, never an array or kit wrapper. */
function isPlainObject(value: DocValue): value is Record<string, DocValue> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  if (value instanceof OrderedObject || value instanceof RawNumber || isExpr(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

/** Validates and copies the value of a mapping's special `extra` key. */
function extraRecord(value: DocValue, path: string): Record<string, DocValue> {
  const entries = mappingEntries(value);
  if (entries === undefined) {
    throw mismatch("an object of extra properties", value, path);
  }
  const rendered: Entry[] = [];
  for (const [key, child] of entries) {
    rendered.push([key, copyDocument(child)]);
  }
  return Object.fromEntries(rendered);
}

/**
 * Appends held-back `extra` entries last, rendering their values without a
 * schema. Errors raised by {@link applyExtra} gain the mapping path.
 */
function appendExtras(
  entries: Entry[],
  extra: Record<string, DocValue> | undefined,
  path: string,
): Entry[] {
  if (extra === undefined) {
    return entries;
  }
  try {
    return applyExtra(entries, extra);
  } catch (error) {
    if (error instanceof LooptError) {
      throw new LooptError(error.code, error.message + at(path));
    }
    throw error;
  }
}

/** Copies an unconstrained document value, rendering every contained expression. */
function copyDocument(value: DocValue): DocValue {
  if (isExpr(value)) {
    return value.toString();
  }
  if (value instanceof RawNumber) {
    return value;
  }
  if (value instanceof OrderedObject) {
    const copied: Entry[] = [];
    for (const [key, child] of value.entries) {
      copied.push([key, copyDocument(child)]);
    }
    return new OrderedObject(copied);
  }
  if (Array.isArray(value)) {
    return value.map(copyDocument);
  }
  if (isPlainObject(value)) {
    const copied: Entry[] = [];
    for (const [key, child] of Object.entries(value)) {
      copied.push([key, copyDocument(child)]);
    }
    return obj(copied);
  }
  return value;
}

/** Formats the ` (at <path>)` suffix shared by every error message. */
function at(path: string): string {
  return " (at " + (path === "" ? "/" : path) + ")";
}

/** Builds a `type-mismatch` error for `value` at `path`. */
function mismatch(expected: string, value: DocValue, path: string): LooptError {
  return new LooptError(
    "type-mismatch",
    "expected " + expected + ", received " + describe(value) + at(path),
  );
}

/** Describes a value's runtime shape for error messages. */
function describe(value: DocValue): string {
  if (value === null) {
    return "null";
  }
  if (isExpr(value)) {
    return "an expression";
  }
  if (value instanceof RawNumber) {
    return "the number literal " + value.literal;
  }
  if (Array.isArray(value)) {
    return "an array";
  }
  if (value instanceof OrderedObject) {
    return "an object";
  }
  switch (typeof value) {
    case "string":
      return "the string " + JSON.stringify(value);
    case "number":
      return "the number " + String(value);
    case "boolean":
      return "the boolean " + String(value);
    default:
      return typeof value;
  }
}
