import { LooptError } from "../kit/index.ts";
import type { Expr } from "../kit/index.ts";
import { context } from "../expr/index.ts";
import type { DynExpr } from "../expr/index.ts";
import { newToken } from "./ref_tokens.ts";

/** Results a step can report through `outcome` and `conclusion`. */
type StepResult = "success" | "failure" | "cancelled" | "skipped";

/** Builds `steps.<token>.<path...>` as a member expression. */
function stepMember(token: string, path: readonly string[]): Expr<unknown> {
  let node: DynExpr = context("steps")[token];
  for (const name of path) {
    node = node[name];
  }
  return node;
}

/** Builds the error raised when a step output was not declared. */
function undeclaredOutput(name: string, declared: readonly string[]): LooptError {
  const listed =
    declared.length === 0
      ? "none"
      : declared.map((output) => JSON.stringify(output)).join(", ");
  return new LooptError(
    "undeclared-output",
    "step output " +
      JSON.stringify(name) +
      " is not declared; declared outputs: " +
      listed,
  );
}

/**
 * A single step definition authored with the helpers in
 * {@link ../steps.ts steps.ts}.
 *
 * The reference keeps its camelCase definition until the surrounding workflow
 * is assembled; targets of `wait` and `cancel` steps may still be step
 * references at that point.
 *
 * Every reference owns a unique token that stands in for its id inside
 * expressions built from it. Assembly assigns a real id when the step is
 * referenced, and the token is replaced once the document is lowered.
 *
 * @typeParam O - Names of the outputs the step declares. The default accepts
 * any output name; a `run` step with no declared outputs has type
 * `StepRef<never>`.
 */
export class StepRef<O extends string = string> {
  /** The kind of step this reference describes. */
  readonly kind: "run" | "uses" | "wait" | "wait-all" | "cancel" | "parallel";

  /** The camelCase step definition built by the step helper. */
  readonly def: { [key: string]: unknown };

  /** Unique placeholder for this step's id inside expressions. */
  readonly token: string;

  /**
   * Names of the outputs this step declares, or `undefined` when the step
   * does not restrict them (the untyped `uses` and `local` fallback).
   */
  readonly declared: readonly string[] | undefined;

  constructor(
    kind: StepRef["kind"],
    def: { [key: string]: unknown },
    declared?: readonly O[],
  ) {
    this.kind = kind;
    this.def = def;
    this.token = newToken("step");
    this.declared = declared;
  }

  /** The explicit `id` carried by the definition, when one was supplied. */
  get id(): string | undefined {
    const id: unknown = this.def.id;
    return typeof id === "string" ? id : undefined;
  }

  /**
   * Typed references to the outputs this step declares.
   *
   * Reading `name` produces `steps.<token>.outputs.<name>`; assembly replaces
   * the token with the step's final id.
   *
   * @throws {LooptError} `"undeclared-output"` when the step declares its
   * outputs and `name` is not one of them.
   */
  get outputs(): { readonly [K in O]: Expr<string> } {
    const token = this.token;
    const declared = this.declared;
    return new Proxy({} as { readonly [K in O]: Expr<string> }, {
      get(_target, property) {
        if (typeof property !== "string") {
          return undefined;
        }
        if (declared !== undefined && !declared.includes(property)) {
          throw undeclaredOutput(property, declared);
        }
        return stepMember(token, ["outputs", property]) as unknown as Expr<string>;
      },
    });
  }

  /**
   * The step's result before `continue-on-error` is applied: `steps.<token>.outcome`.
   */
  get outcome(): Expr<StepResult> {
    return stepMember(this.token, ["outcome"]) as unknown as Expr<StepResult>;
  }

  /**
   * The step's result after `continue-on-error` is applied:
   * `steps.<token>.conclusion`.
   */
  get conclusion(): Expr<StepResult> {
    return stepMember(this.token, ["conclusion"]) as unknown as Expr<StepResult>;
  }
}

/** Alias for {@link StepRef}, the SDK's step value. */
export type Step = StepRef;
