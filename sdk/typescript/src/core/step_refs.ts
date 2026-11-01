import { LooptError } from "../kit/index.ts";
import { collectTokens, tokenKind } from "./ref_tokens.ts";
import { StepRef } from "./step_ref.ts";
import type { AssembleContext } from "./passes.ts";

/** Where one step sits inside the workflow being assembled. */
interface StepSite {
  /** The step reference itself. */
  readonly step: StepRef;
  /** Id of the job that contains the step. */
  readonly jobId: string;
  /** 1-based position in the job's depth-first step enumeration. */
  readonly position: number;
}

/** Returns the set of ids already used in `jobId`, creating it when absent. */
function jobIdSet(
  usedIds: Map<string, Set<string>>,
  jobId: string,
): Set<string> {
  const existing = usedIds.get(jobId);
  if (existing !== undefined) {
    return existing;
  }
  const created = new Set<string>();
  usedIds.set(jobId, created);
  return created;
}

/**
 * Enumerates `step` and its parallel children depth-first, recording each
 * step's token, owning job, and 1-based position.
 *
 * @throws {LooptError} `"invalid-step"` when a parallel value is not an array
 * of step references.
 */
function enumerateSteps(
  step: StepRef,
  jobId: string,
  counter: { value: number },
  sites: Map<string, StepSite>,
): void {
  counter.value += 1;
  sites.set(step.token, { step, jobId, position: counter.value });
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
    enumerateSteps(item, jobId, counter, sites);
  }
}

/** Records the step tokens referenced by `wait` and `cancel` targets. */
function collectTargetTokens(value: unknown, tokens: Set<string>): void {
  if (value instanceof StepRef) {
    tokens.add(value.token);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      collectTargetTokens(item, tokens);
    }
  }
}

/**
 * Records every step token referenced by `step`: tokens in its definition
 * values, `wait` and `cancel` targets, and the definitions of its parallel
 * children.
 *
 * @throws {LooptError} `"invalid-step"` when a parallel value is not an array
 * of step references.
 */
function collectStepTokens(step: StepRef, tokens: Set<string>): void {
  for (const [key, value] of Object.entries(step.def)) {
    if (key === "wait" || key === "cancel") {
      collectTargetTokens(value, tokens);
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
      for (const item of inner) {
        if (!(item instanceof StepRef)) {
          throw new LooptError(
            "invalid-step",
            "a parallel step must hold StepRef values",
          );
        }
        collectStepTokens(item, tokens);
      }
      continue;
    }
    for (const token of collectTokens(value)) {
      tokens.add(token);
    }
  }
}

/**
 * Assigns an id to `site` when it is referenced and has none yet.
 *
 * The first candidate is `step_<position>`; when that id is already taken in
 * the job, `_1`, `_2`, and so on are appended until one is free. The chosen id
 * is recorded in `context.stepIds`, which the lowering pass reads.
 */
function ensureStepId(
  site: StepSite,
  context: AssembleContext,
  usedIds: Map<string, Set<string>>,
): void {
  if (context.stepIds.has(site.step)) {
    return;
  }
  const ids = jobIdSet(usedIds, site.jobId);
  const base = "step_" + String(site.position);
  let id = base;
  let suffix = 0;
  while (ids.has(id)) {
    suffix += 1;
    id = base + "_" + String(suffix);
  }
  ids.add(id);
  context.stepIds.set(site.step, id);
}

/**
 * Assigns ids to the steps referenced from expressions.
 *
 * A step referenced by another step or by its own job is given an id unless
 * it already declares one, so `meta.outputs.version` can lower to
 * `steps.step_1.outputs.version`. Explicit ids are preserved and generated
 * ids stay unique within each job.
 *
 * @throws {LooptError} `"unknown-step-reference"` when a reference points at
 * a step outside this workflow.
 * @throws {LooptError} `"step-ref-cross-job"` when a job references a step of
 * a different job.
 */
export function assignStepIds(context: AssembleContext): void {
  const sites = new Map<string, StepSite>();
  for (const job of context.jobs) {
    const counter = { value: 0 };
    for (const step of job.steps) {
      enumerateSteps(step, job.id, counter, sites);
    }
  }

  const usedIds = new Map<string, Set<string>>();
  for (const site of sites.values()) {
    const explicit = context.stepIds.get(site.step);
    if (explicit !== undefined) {
      jobIdSet(usedIds, site.jobId).add(explicit);
    }
  }

  for (const job of context.jobs) {
    const tokens = new Set<string>();
    for (const [key, value] of Object.entries(job.def)) {
      if (key === "steps") {
        continue;
      }
      for (const token of collectTokens(value)) {
        tokens.add(token);
      }
    }
    for (const step of job.steps) {
      collectStepTokens(step, tokens);
    }
    for (const token of tokens) {
      if (tokenKind(token) !== "step") {
        continue;
      }
      const site = sites.get(token);
      if (site === undefined) {
        throw new LooptError(
          "unknown-step-reference",
          "a step reference in job " +
            JSON.stringify(job.id) +
            " points at a step that is not part of this workflow",
        );
      }
      if (site.jobId !== job.id) {
        throw new LooptError(
          "step-ref-cross-job",
          "a step reference in job " +
            JSON.stringify(job.id) +
            " points at a step of job " +
            JSON.stringify(site.jobId) +
            "; expose that step's value through the owning job's outputs instead",
        );
      }
      ensureStepId(site, context, usedIds);
    }
  }
}
