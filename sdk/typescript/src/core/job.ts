import type { JobFactory, WorkflowJob } from "../generated/workflow.ts";
import { LooptError } from "../kit/index.ts";
import type { DocValue } from "../kit/index.ts";
import { JobRef } from "./job_ref.ts";
import type { StepRef } from "./step_ref.ts";

/**
 * A job definition accepted by {@link job}: the generated job-factory fields
 * with SDK-friendly `needs` and `steps`.
 */
export type JobDef = Omit<JobFactory, "needs" | "steps" | "extra"> & {
  /** Jobs that must complete before this one; references lower to their ids. */
  needs?: JobRef | string | Array<JobRef | string>;
  /** Steps in execution order; nested arrays are flattened by assembly. */
  steps: Array<StepRef | StepRef[]>;
  /** Extra keys appended after the canonical keys, without schema processing. */
  extra?: Record<string, DocValue>;
};

/**
 * A reusable-workflow call definition accepted by {@link callWorkflow}: the
 * generated workflow-job fields without `uses`, with SDK-friendly `needs`.
 */
export type CallDef = Omit<WorkflowJob, "uses" | "needs"> & {
  /** Jobs that must complete before this one; references lower to their ids. */
  needs?: JobRef | string | Array<JobRef | string>;
};

/** Copies an authored definition into a plain record, preserving key order. */
function definitionRecord(def: object): { [key: string]: unknown } {
  const record: { [key: string]: unknown } = {};
  for (const [key, value] of Object.entries(def)) {
    record[key] = value;
  }
  return record;
}

/**
 * Builds a regular job from its camelCase definition.
 *
 * Nothing beyond the argument shape is validated here; the definition is
 * checked against the workflow schema when the surrounding workflow produces
 * its document.
 */
export function job(def: JobDef): JobRef {
  return new JobRef("job", definitionRecord(def));
}

/**
 * Builds a reusable-workflow call job.
 *
 * The lowered definition places `uses` first, then the remaining definition
 * keys in their own order.
 *
 * @throws {LooptError} `"duplicate-key"` when `def` also carries `uses`.
 */
export function callWorkflow(uses: string, def?: CallDef): JobRef {
  const lowered: { [key: string]: unknown } = { uses };
  if (def !== undefined) {
    for (const [key, value] of Object.entries(def)) {
      if (key === "uses") {
        throw new LooptError(
          "duplicate-key",
          "callWorkflow() received `uses` both as an argument and in the definition",
        );
      }
      lowered[key] = value;
    }
  }
  return new JobRef("call", lowered);
}
