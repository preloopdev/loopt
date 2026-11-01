import { LooptError, obj } from "../kit/index.ts";
import type { DocValue, Expr } from "../kit/index.ts";
import { StepRef } from "./step_ref.ts";

/** Options shared by the run, uses, and wait-all step helpers. */
export interface StepOpts {
  /** Step name displayed by GitHub. */
  name?: string | Expr;
  /** Step id, used by `wait`, `cancel`, and step outputs. */
  id?: string;
  /** Conditional that must hold for the step to run. */
  if?: Expr<boolean> | boolean;
  /** Environment variables available to the step. */
  env?: Record<string, string | number | boolean | Expr>;
  /** Minutes to wait before killing the step. */
  timeoutMinutes?: number | Expr;
  /** Whether a failure of this step fails the job. */
  continueOnError?: boolean | Expr;
  /** Whether the step runs in the background. */
  background?: boolean;
  /** Extra keys appended after the canonical keys, without schema processing. */
  extra?: Record<string, DocValue>;
}

/** Options accepted by {@link run}, adding the shell and working directory. */
export interface RunStepOpts extends StepOpts {
  /** Shell used to run the command. */
  shell?: string;
  /** Directory the command runs in. */
  workingDirectory?: string | Expr;
}

/** Options accepted by {@link waitAll}. */
export type WaitAllOpts = Pick<StepOpts, "name" | "id" | "continueOnError">;

/** Builds a step reference from canonical, ordered entries. */
function stepRef(kind: StepRef["kind"], entries: Array<[string, unknown]>): StepRef {
  const def: { [key: string]: unknown } = {};
  for (const [key, value] of entries) {
    def[key] = value;
  }
  return new StepRef(kind, def);
}

/** Copies a user-supplied record into a document value, preserving key order. */
function recordValue(
  values: Record<string, string | number | boolean | Expr>,
): DocValue {
  const entries: Array<[string, DocValue]> = [];
  for (const [key, value] of Object.entries(values)) {
    entries.push([key, value]);
  }
  return obj(entries);
}

/** Appends `[key, value]` when the value is defined. */
function pushDefined(
  entries: Array<[string, unknown]>,
  key: string,
  value: unknown,
): void {
  if (value !== undefined) {
    entries.push([key, value]);
  }
}

/**
 * Builds a step that runs a shell command.
 *
 * Keys are emitted in the order `name`, `id`, `if`, `run`, `env`, `shell`,
 * `workingDirectory`, `continueOnError`, `timeoutMinutes`, `background`,
 * `extra`.
 */
export function run(command: string, opts?: RunStepOpts): StepRef {
  const entries: Array<[string, unknown]> = [];
  if (opts !== undefined) {
    pushDefined(entries, "name", opts.name);
    pushDefined(entries, "id", opts.id);
    pushDefined(entries, "if", opts.if);
  }
  entries.push(["run", command]);
  if (opts !== undefined) {
    if (opts.env !== undefined) {
      entries.push(["env", recordValue(opts.env)]);
    }
    pushDefined(entries, "shell", opts.shell);
    pushDefined(entries, "workingDirectory", opts.workingDirectory);
    pushDefined(entries, "continueOnError", opts.continueOnError);
    pushDefined(entries, "timeoutMinutes", opts.timeoutMinutes);
    pushDefined(entries, "background", opts.background);
    pushDefined(entries, "extra", opts.extra);
  }
  return stepRef("run", entries);
}

/**
 * Builds a step that runs an action.
 *
 * Keys are emitted in the order `name`, `id`, `if`, `uses`, `with`, `env`,
 * `continueOnError`, `timeoutMinutes`, `background`, `extra`; `with` is
 * omitted when `inputs` is absent or empty.
 */
export function uses(
  action: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts,
): StepRef {
  const entries: Array<[string, unknown]> = [];
  if (opts !== undefined) {
    pushDefined(entries, "name", opts.name);
    pushDefined(entries, "id", opts.id);
    pushDefined(entries, "if", opts.if);
  }
  entries.push(["uses", action]);
  if (inputs !== undefined && Object.keys(inputs).length > 0) {
    entries.push(["with", recordValue(inputs)]);
  }
  if (opts !== undefined) {
    if (opts.env !== undefined) {
      entries.push(["env", recordValue(opts.env)]);
    }
    pushDefined(entries, "continueOnError", opts.continueOnError);
    pushDefined(entries, "timeoutMinutes", opts.timeoutMinutes);
    pushDefined(entries, "background", opts.background);
    pushDefined(entries, "extra", opts.extra);
  }
  return stepRef("uses", entries);
}

/** Builds a step that runs a Docker image as an action (`docker://<image>`). */
export function docker(
  image: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts,
): StepRef {
  return uses("docker://" + image, inputs, opts);
}

/**
 * Builds a step that runs an action from the local repository.
 *
 * @throws {LooptError} `"invalid-local-path"` when `path` does not start with
 * `"./"`.
 */
export function local(
  path: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts,
): StepRef {
  if (!path.startsWith("./")) {
    throw new LooptError(
      "invalid-local-path",
      "local() expects a repository-relative action path starting with \"./\", received " +
        JSON.stringify(path),
    );
  }
  return uses(path, inputs, opts);
}

/**
 * Builds a step that waits for background steps identified by `targets`.
 *
 * A single target lowers to one id; several targets lower to an array.
 */
export function wait(...targets: Array<StepRef | string>): StepRef {
  const value: unknown = targets.length === 1 ? targets[0] : targets;
  return new StepRef("wait", { wait: value });
}

/** Builds a step that waits for all prior background steps to complete. */
export function waitAll(opts?: WaitAllOpts): StepRef {
  const entries: Array<[string, unknown]> = [];
  if (opts !== undefined) {
    pushDefined(entries, "name", opts.name);
    pushDefined(entries, "id", opts.id);
    pushDefined(entries, "continueOnError", opts.continueOnError);
  }
  entries.push(["waitAll", null]);
  return stepRef("wait-all", entries);
}

/** Builds a step that cancels a background step. */
export function cancel(target: StepRef | string): StepRef {
  return new StepRef("cancel", { cancel: target });
}

/** Builds a step that runs `steps` concurrently and waits for all of them. */
export function parallel(...steps: StepRef[]): StepRef {
  return new StepRef("parallel", { parallel: steps });
}
