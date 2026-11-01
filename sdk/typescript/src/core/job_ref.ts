import { LooptError, OrderedObject } from "../kit/index.ts";
import type { Expr } from "../kit/index.ts";
import { context } from "../expr/index.ts";
import type { DynExpr } from "../expr/index.ts";
import { newToken } from "./ref_tokens.ts";

/** Results a job can report through `needs.<id>.result`. */
type JobResult = "success" | "failure" | "cancelled" | "skipped";

/** Builds `needs.<token>.<path...>` as a member expression. */
function jobMember(token: string, path: readonly string[]): Expr<unknown> {
  let node: DynExpr = context("needs")[token];
  for (const name of path) {
    node = node[name];
  }
  return node;
}

/** Returns the output names declared by a job definition's `outputs` mapping. */
function declaredOutputs(outputs: unknown): string[] {
  if (outputs instanceof OrderedObject) {
    return outputs.entries.map(([name]) => name);
  }
  if (
    typeof outputs === "object" &&
    outputs !== null &&
    !Array.isArray(outputs)
  ) {
    return Object.keys(outputs);
  }
  return [];
}

/** Builds the error raised when a job output was not declared. */
function undeclaredOutput(name: string, declared: readonly string[]): LooptError {
  const listed =
    declared.length === 0
      ? "none"
      : declared.map((output) => JSON.stringify(output)).join(", ");
  return new LooptError(
    "undeclared-output",
    "job output " +
      JSON.stringify(name) +
      " is not declared; declared outputs: " +
      listed,
  );
}

/**
 * A job definition that becomes a key under `jobs:` once placed in a
 * {@link ../workflow.ts Workflow}.
 *
 * The reference keeps the authored camelCase definition until assembly lowers
 * it; the definition may still hold {@link ../step_ref.ts StepRef} values and
 * job references in `needs`.
 *
 * Every reference owns a unique token that stands in for its id inside
 * expressions built from it. Assembly replaces the token with the job's final
 * id once the document is lowered.
 *
 * @typeParam O - Names of the outputs the job declares. The default accepts
 * any output name; a job built without `outputs` has type `JobRef<never>`.
 */
export class JobRef<O extends string = string> {
  /** Whether this reference is a regular job or a reusable-workflow call. */
  readonly kind: "job" | "call";

  /**
   * The camelCase job definition exactly as given: steps are still
   * `StepRef`s and `needs` still holds job references.
   */
  readonly def: { [key: string]: unknown };

  /** Unique placeholder for this job's id inside expressions. */
  readonly token: string;

  /** The id this job is placed under in a workflow, once assembled. */
  placedId: string | undefined;

  constructor(kind: "job" | "call", def: { [key: string]: unknown }) {
    this.kind = kind;
    this.def = def;
    this.token = newToken("job");
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

  /**
   * Typed references to the outputs this job declares.
   *
   * Reading `name` produces `needs.<token>.outputs.<name>`; assembly replaces
   * the token with the job's final id. Regular jobs may only read the outputs
   * declared in their `outputs` mapping, while reusable-workflow calls accept
   * any name.
   *
   * @throws {LooptError} `"undeclared-output"` when this is a regular job and
   * `name` is not one of its declared outputs.
   */
  get outputs(): { readonly [K in O]: Expr<string> } {
    const token = this.token;
    const declared =
      this.kind === "job" ? declaredOutputs(this.def.outputs) : undefined;
    return new Proxy({} as { readonly [K in O]: Expr<string> }, {
      get(_target, property) {
        if (typeof property !== "string") {
          return undefined;
        }
        if (declared !== undefined && !declared.includes(property)) {
          throw undeclaredOutput(property, declared);
        }
        return jobMember(token, ["outputs", property]) as unknown as Expr<string>;
      },
    });
  }

  /** The job's result: `needs.<token>.result`. */
  get result(): Expr<JobResult> {
    return jobMember(this.token, ["result"]) as unknown as Expr<JobResult>;
  }
}
