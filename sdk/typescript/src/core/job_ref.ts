import { LooptError } from "../kit/index.ts";

/**
 * A job definition that becomes a key under `jobs:` once placed in a
 * {@link ../workflow.ts Workflow}.
 *
 * The reference keeps the authored camelCase definition until assembly lowers
 * it; the definition may still hold {@link ../step_ref.ts StepRef} values and
 * job references in `needs`.
 */
export class JobRef {
  /** Whether this reference is a regular job or a reusable-workflow call. */
  readonly kind: "job" | "call";

  /**
   * The camelCase job definition exactly as given: steps are still
   * `StepRef`s and `needs` still holds job references.
   */
  readonly def: { [key: string]: unknown };

  /** The id this job is placed under in a workflow, once assembled. */
  placedId: string | undefined;

  constructor(kind: "job" | "call", def: { [key: string]: unknown }) {
    this.kind = kind;
    this.def = def;
    this.placedId = undefined;
  }

  /**
   * The id this job was placed under in a workflow.
   *
   * @throws {LooptError} `"unplaced-job"` when the job has not been placed in a
   * workflow yet.
   */
  get id(): string {
    if (this.placedId === undefined) {
      throw new LooptError(
        "unplaced-job",
        "this job has not been placed in a workflow, so it has no id",
      );
    }
    return this.placedId;
  }
}
