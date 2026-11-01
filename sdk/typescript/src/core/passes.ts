import { LooptError, OrderedObject, obj } from "../kit/index.ts";
import type { DocValue } from "../kit/index.ts";
import { JobRef } from "./job_ref.ts";
import { MatrixRef, matrixDefinition } from "./matrix.ts";
import { normalizeWorkflow } from "./normalize.ts";
import { StepRef } from "./step_ref.ts";

/**
 * Mutable state threaded through the assembly passes.
 *
 * Early passes fill `def`, `steps`, `stepIds`, and `needs`; the final pass
 * stores the lowered camelCase workflow document in `doc`.
 */
export interface AssembleContext {
  /** Workflow name; omitted from the document when undefined. */
  workflowName: string | undefined;
  /** Workflow definition without `jobs`, camelCase, in its own key order. */
  rest: { [key: string]: unknown };
  /** Jobs in declaration order, with their working copies and flattened steps. */
  jobs: Array<{
    id: string;
    ref: JobRef;
    def: { [key: string]: unknown };
    steps: StepRef[];
  }>;
  /** Step ids by step; explicit ids are pre-populated by `collectExplicit`. */
  stepIds: Map<StepRef, string>;
  /** Job dependencies by job; explicit needs are pre-populated by `collectExplicit`. */
  needs: Map<JobRef, Array<JobRef | string>>;
  /** Lowered camelCase document root, set by the final pass. */
  doc: { [key: string]: unknown } | undefined;
}

/** A single assembly pass over an {@link AssembleContext}. */
export type Pass = (context: AssembleContext) => void;

/** Copies an authored definition into a plain record, preserving key order. */
function copyDefinition(def: { [key: string]: unknown }): { [key: string]: unknown } {
  const copy: { [key: string]: unknown } = {};
  for (const [key, value] of Object.entries(def)) {
    copy[key] = value;
  }
  return copy;
}

/** Builds a plain record from ordered entries. */
function record(entries: Array<[string, unknown]>): { [key: string]: unknown } {
  const result: { [key: string]: unknown } = {};
  for (const [key, value] of entries) {
    result[key] = value;
  }
  return result;
}

/** Returns the entries of a plain object or {@link OrderedObject}. */
function mappingEntries(value: unknown): Array<[string, unknown]> | undefined {
  if (value instanceof OrderedObject) {
    return value.entries.map(([key, entry]): [string, unknown] => [key, entry]);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  const prototype: unknown = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    return undefined;
  }
  return Object.entries(value);
}

/** Inserts `entry` after the first `key`, or appends it when `key` is absent. */
function insertAfter(
  entries: Array<[string, unknown]>,
  key: string,
  entry: [string, unknown],
): void {
  const index = entries.findIndex(([name]) => name === key);
  if (index === -1) {
    entries.push(entry);
  } else {
    entries.splice(index + 1, 0, entry);
  }
}

/** Records a step as used, rejecting a second use in the same workflow. */
function registerStep(step: StepRef, seen: Set<StepRef>): void {
  if (seen.has(step)) {
    throw new LooptError(
      "step-reused",
      "the same step reference is used more than once in the workflow",
    );
  }
  seen.add(step);
}

/** Registers a step's parallel children so reuse across the workflow is detected. */
function registerParallelSteps(step: StepRef, seen: Set<StepRef>): void {
  if (step.kind !== "parallel") {
    return;
  }
  const inner: unknown = step.def.parallel;
  if (!Array.isArray(inner)) {
    throw new LooptError(
      "invalid-step",
      "a parallel step must hold an array of steps",
    );
  }
  for (const item of inner) {
    if (!(item instanceof StepRef)) {
      throw new LooptError(
        "invalid-step",
        "a parallel step must hold StepRef values",
      );
    }
    registerStep(item, seen);
    registerParallelSteps(item, seen);
  }
}

/** Flattens nested step arrays, rejecting non-step values. */
function collectSteps(
  value: unknown,
  flat: StepRef[],
  seen: Set<StepRef>,
): void {
  if (!Array.isArray(value)) {
    throw new LooptError(
      "invalid-step",
      "job steps must be an array of StepRef values",
    );
  }
  for (const item of value) {
    if (Array.isArray(item)) {
      collectSteps(item, flat, seen);
      continue;
    }
    if (!(item instanceof StepRef)) {
      throw new LooptError(
        "invalid-step",
        "job steps must be an array of StepRef values",
      );
    }
    registerStep(item, seen);
    flat.push(item);
    registerParallelSteps(item, seen);
  }
}

/**
 * Places every job reference under its record key, setting `placedId`.
 *
 * @throws {LooptError} `"invalid-job"` when a value is not a {@link JobRef}.
 * @throws {LooptError} `"job-placed-twice"` when one reference appears under
 * two ids.
 */
export function assignJobIds(context: AssembleContext): void {
  const placements = new Map<JobRef, string>();
  for (const entry of context.jobs) {
    const ref: unknown = entry.ref;
    if (!(ref instanceof JobRef)) {
      throw new LooptError(
        "invalid-job",
        "job " + JSON.stringify(entry.id) + " is not a JobRef",
      );
    }
    const existing = placements.get(ref);
    if (existing !== undefined) {
      throw new LooptError(
        "job-placed-twice",
        "the JobRef placed as " +
          JSON.stringify(existing) +
          " is placed again as " +
          JSON.stringify(entry.id),
      );
    }
    placements.set(ref, entry.id);
  }
  for (const [ref, id] of placements) {
    ref.placedId = id;
  }
}

/**
 * Copies each job definition and flattens its step arrays into `steps`.
 *
 * @throws {LooptError} `"invalid-step"` for non-step values.
 * @throws {LooptError} `"step-reused"` when a step is used twice in the
 * workflow.
 */
export function flattenSteps(context: AssembleContext): void {
  const seen = new Set<StepRef>();
  for (const job of context.jobs) {
    const def = copyDefinition(job.ref.def);
    const flat: StepRef[] = [];
    const rawSteps: unknown = def.steps;
    if (rawSteps !== undefined) {
      collectSteps(rawSteps, flat, seen);
      def.steps = flat;
    }
    job.def = def;
    job.steps = flat;
  }
}

/** Collects a step's explicit id, rejecting duplicates within one job. */
function collectStepId(
  step: StepRef,
  jobId: string,
  ids: Set<string>,
  stepIds: Map<StepRef, string>,
): void {
  const id = step.id;
  if (id !== undefined) {
    if (ids.has(id)) {
      throw new LooptError(
        "duplicate-step-id",
        "step id " +
          JSON.stringify(id) +
          " is used more than once in job " +
          JSON.stringify(jobId),
      );
    }
    ids.add(id);
    stepIds.set(step, id);
  }
  if (step.kind === "parallel") {
    const inner: unknown = step.def.parallel;
    if (!Array.isArray(inner)) {
      throw new LooptError(
        "invalid-step",
        "a parallel step must hold an array of steps",
      );
    }
    for (const item of inner) {
      if (!(item instanceof StepRef)) {
        throw new LooptError(
          "invalid-step",
          "a parallel step must hold StepRef values",
        );
      }
      collectStepId(item, jobId, ids, stepIds);
    }
  }
}

/** Reads a job's explicit `needs` as a list of references and ids. */
function explicitNeeds(value: unknown): Array<JobRef | string> | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (Array.isArray(value)) {
    const targets: Array<JobRef | string> = [];
    for (const item of value) {
      targets.push(item as JobRef | string);
    }
    return targets;
  }
  return [value as JobRef | string];
}

/**
 * Records explicit step ids and job dependencies for later passes.
 *
 * @throws {LooptError} `"duplicate-step-id"` when one job repeats an id.
 * @throws {LooptError} `"unknown-job-reference"` when `needs` names a job that
 * is not part of this workflow.
 */
export function collectExplicit(context: AssembleContext): void {
  const members = new Set<JobRef>();
  for (const job of context.jobs) {
    members.add(job.ref);
  }
  for (const job of context.jobs) {
    const ids = new Set<string>();
    for (const step of job.steps) {
      collectStepId(step, job.id, ids, context.stepIds);
    }
    const needs = explicitNeeds(job.def.needs);
    if (needs === undefined) {
      continue;
    }
    for (const target of needs) {
      if (target instanceof JobRef && !members.has(target)) {
        throw new LooptError(
          "unknown-job-reference",
          "job " +
            JSON.stringify(job.id) +
            " needs a job that is not part of this workflow",
        );
      }
    }
    context.needs.set(job.ref, needs);
  }
}

/** Lowers a wait or cancel target to its step id. */
function lowerTarget(value: unknown, kind: "wait" | "cancel"): string {
  if (typeof value === "string") {
    return value;
  }
  if (value instanceof StepRef) {
    const id = value.id;
    if (id === undefined) {
      throw new LooptError(
        "missing-step-id",
        kind + " target step needs an explicit id",
      );
    }
    return id;
  }
  throw new LooptError(
    "invalid-step",
    kind + " target must be a StepRef or a step id string",
  );
}

/** Lowers a `wait` value to one id or an array of ids. */
function lowerWaitTargets(value: unknown): string | string[] {
  if (Array.isArray(value)) {
    const ids: string[] = [];
    for (const item of value) {
      ids.push(lowerTarget(item, "wait"));
    }
    return ids;
  }
  return lowerTarget(value, "wait");
}

/** Lowers a step definition, including nested parallel and wait targets. */
function lowerStep(step: StepRef, context: AssembleContext): { [key: string]: unknown } {
  const entries: Array<[string, unknown]> = [];
  for (const [key, value] of Object.entries(step.def)) {
    if (key === "wait") {
      entries.push([key, lowerWaitTargets(value)]);
      continue;
    }
    if (key === "cancel") {
      entries.push([key, lowerTarget(value, "cancel")]);
      continue;
    }
    if (key === "parallel") {
      const inner: unknown = value;
      if (!Array.isArray(inner)) {
        throw new LooptError(
          "invalid-step",
          "a parallel step must hold an array of steps",
        );
      }
      const lowered: Array<{ [key: string]: unknown }> = [];
      for (const item of inner) {
        if (!(item instanceof StepRef)) {
          throw new LooptError(
            "invalid-step",
            "a parallel step must hold StepRef values",
          );
        }
        lowered.push(lowerStep(item, context));
      }
      entries.push([key, lowered]);
      continue;
    }
    entries.push([key, value]);
  }
  const id = context.stepIds.get(step);
  if (id !== undefined) {
    const index = entries.findIndex(([key]) => key === "id");
    if (index === -1) {
      const nameIndex = entries.findIndex(([key]) => key === "name");
      entries.splice(nameIndex === -1 ? 0 : nameIndex + 1, 0, ["id", id]);
    } else {
      entries[index] = ["id", id];
    }
  }
  return record(entries);
}

/** Lowers a job's strategy, replacing a {@link MatrixRef} with its definition. */
function lowerStrategy(value: unknown): unknown {
  const entries = mappingEntries(value);
  if (entries === undefined) {
    return value;
  }
  const lowered: Array<[string, unknown]> = [];
  for (const [key, entry] of entries) {
    if (key === "matrix" && entry instanceof MatrixRef) {
      lowered.push([key, matrixDefinition(entry)]);
      continue;
    }
    lowered.push([key, entry]);
  }
  return record(lowered);
}

/** Lowers a job's dependencies to deduplicated job ids. */
function lowerNeeds(
  ref: JobRef,
  needs: Map<JobRef, Array<JobRef | string>>,
): string | string[] | undefined {
  const targets = needs.get(ref);
  if (targets === undefined) {
    return undefined;
  }
  const ids: string[] = [];
  for (const target of targets) {
    const id = typeof target === "string" ? target : target.id;
    if (!ids.includes(id)) {
      ids.push(id);
    }
  }
  return ids.length === 1 ? ids[0] : ids;
}

/** Lowers a job definition, replacing steps, needs, and matrix references. */
function lowerJob(
  job: AssembleContext["jobs"][number],
  context: AssembleContext,
): { [key: string]: unknown } {
  const needs = lowerNeeds(job.ref, context.needs);
  const entries: Array<[string, unknown]> = [];
  let needsPlaced = false;
  for (const [key, value] of Object.entries(job.def)) {
    if (key === "steps") {
      const lowered: Array<{ [key: string]: unknown }> = [];
      for (const step of job.steps) {
        lowered.push(lowerStep(step, context));
      }
      entries.push([key, lowered]);
      continue;
    }
    if (key === "needs") {
      if (needs !== undefined) {
        entries.push([key, needs]);
        needsPlaced = true;
      }
      continue;
    }
    if (key === "strategy") {
      entries.push([key, lowerStrategy(value)]);
      continue;
    }
    entries.push([key, value]);
  }
  if (needs !== undefined && !needsPlaced) {
    insertAfter(entries, "runsOn", ["needs", needs]);
  }
  return record(entries);
}

/**
 * Builds the lowered camelCase workflow document and stores it on `context`.
 *
 * The document places `name` first when defined, then the workflow
 * definition, then `jobs` in declaration order. Jobs replace their step
 * references with lowered step definitions and their job references with ids.
 */
export function lowerWorkflow(context: AssembleContext): void {
  const rootEntries: Array<[string, unknown]> = [];
  if (context.workflowName !== undefined) {
    rootEntries.push(["name", context.workflowName]);
  }
  for (const [key, value] of Object.entries(context.rest)) {
    if (key === "jobs") {
      continue;
    }
    rootEntries.push([key, value]);
  }
  const jobEntries: Array<[string, DocValue]> = [];
  for (const job of context.jobs) {
    jobEntries.push([job.id, lowerJob(job, context) as unknown as DocValue]);
  }
  rootEntries.push(["jobs", obj(jobEntries)]);
  context.doc = record(rootEntries);
}

/** Ordered passes that turn an authored workflow into a document. */
export const PASSES: Pass[] = [
  assignJobIds,
  flattenSteps,
  collectExplicit,
  lowerWorkflow,
];

/**
 * Assembles a workflow from its name, definition, and jobs, then normalizes
 * the result against the strict workflow schema.
 *
 * @throws {LooptError} any pass error, or any normalization error produced by
 * the schema checks.
 */
export function assemble(
  workflowName: string | undefined,
  rest: { [key: string]: unknown },
  jobs: Record<string, JobRef>,
): DocValue {
  const entries: AssembleContext["jobs"] = [];
  for (const [id, ref] of Object.entries(jobs)) {
    entries.push({ id, ref, def: {}, steps: [] });
  }
  const context: AssembleContext = {
    workflowName,
    rest,
    jobs: entries,
    stepIds: new Map<StepRef, string>(),
    needs: new Map<JobRef, Array<JobRef | string>>(),
    doc: undefined,
  };
  for (const pass of PASSES) {
    pass(context);
  }
  const doc = context.doc;
  if (doc === undefined) {
    throw new LooptError(
      "missing-document",
      "the workflow passes did not produce a document",
    );
  }
  return normalizeWorkflow(doc as unknown as { [key: string]: DocValue });
}
