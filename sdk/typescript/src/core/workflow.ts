import type { WorkflowRootStrict } from "../generated/workflow.ts";
import type { DocValue, Documentable } from "../kit/index.ts";
import type { JobRef } from "./job_ref.ts";
import { assemble } from "./passes.ts";

/**
 * A workflow definition accepted by {@link workflow}: the generated strict
 * workflow fields with SDK-friendly `jobs`.
 */
export type WorkflowDef = Omit<WorkflowRootStrict, "name" | "jobs" | "extra"> & {
  /** Jobs keyed by the id they are placed under. */
  jobs: Record<string, JobRef>;
  /** Extra root keys appended last, without schema processing. */
  extra?: Record<string, DocValue>;
};

/**
 * An authored workflow that emits its normalized JSON document on demand.
 *
 * Assembly never mutates the workflow definition or the job and step
 * references it contains, so `toDocument()` can be called repeatedly.
 */
export class Workflow implements Documentable {
  /** Workflow name, emitted first when defined. */
  readonly name: string | undefined;

  /** The authored workflow definition, including its jobs. */
  readonly def: WorkflowDef;

  constructor(name: string | undefined, def: WorkflowDef) {
    this.name = name;
    this.def = def;
  }

  /** Assembles and normalizes the workflow into its deployment document. */
  toDocument(): DocValue {
    const rest: { [key: string]: unknown } = {};
    for (const [key, value] of Object.entries(this.def)) {
      if (key === "jobs") {
        continue;
      }
      rest[key] = value;
    }
    return assemble(this.name, rest, this.def.jobs);
  }
}

/** Builds a workflow document from its name and definition. */
export function workflow(name: string | undefined, def: WorkflowDef): Workflow {
  return new Workflow(name, def);
}
