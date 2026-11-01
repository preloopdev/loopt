/**
 * A single step definition authored with the helpers in
 * {@link ../steps.ts steps.ts}.
 *
 * The reference keeps its camelCase definition until the surrounding workflow
 * is assembled; targets of `wait` and `cancel` steps may still be step
 * references at that point.
 */
export class StepRef {
  /** The kind of step this reference describes. */
  readonly kind: "run" | "uses" | "wait" | "wait-all" | "cancel" | "parallel";

  /** The camelCase step definition built by the step helper. */
  readonly def: { [key: string]: unknown };

  constructor(kind: StepRef["kind"], def: { [key: string]: unknown }) {
    this.kind = kind;
    this.def = def;
  }

  /** The explicit `id` carried by the definition, when one was supplied. */
  get id(): string | undefined {
    const id: unknown = this.def.id;
    return typeof id === "string" ? id : undefined;
  }
}

/** Alias for {@link StepRef}, the SDK's step value. */
export type Step = StepRef;
