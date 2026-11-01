import { LooptError, isExpr, obj } from "../kit/index.ts";
import type { DocValue, Expr } from "../kit/index.ts";
import { StepRef } from "./step_ref.ts";

/**
 * Options shared by the run, uses, and wait-all step helpers.
 *
 * @typeParam O - Names of the outputs the step declares, enabling typed
 * references through {@link StepRef.outputs}.
 */
export interface StepOpts<O extends string = string> {
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
  /**
   * Names of the outputs the step writes. The names are recorded on the
   * reference for typed output access and are never emitted as a step key.
   */
  outputs?: readonly O[];
  /** Extra keys appended after the canonical keys, without schema processing. */
  extra?: Record<string, DocValue>;
}

/** Options accepted by {@link run}, adding the shell and working directory. */
export interface RunStepOpts<O extends string = string> extends StepOpts<O> {
  /** Shell used to run the command. */
  shell?: string;
  /** Directory the command runs in. */
  workingDirectory?: string | Expr;
}

/** Options accepted by {@link waitAll}. */
export type WaitAllOpts = Pick<StepOpts, "name" | "id" | "continueOnError">;

/** Builds a step reference from canonical, ordered entries. */
function stepRef<O extends string>(
  kind: StepRef["kind"],
  entries: Array<[string, unknown]>,
  declared?: readonly O[],
): StepRef<O> {
  const def: { [key: string]: unknown } = {};
  for (const [key, value] of entries) {
    def[key] = value;
  }
  return new StepRef<O>(kind, def, declared);
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
 * Values accepted by the safe tagged `run` template. Expressions stay as
 * objects until workflow lowering so step and job references are resolved.
 */
export type RunTemplateValue = string | number | boolean | Expr;

/** A safe tagged run template with a fixed set of step options. */
export type RunTemplate<O extends string = never> = (
  strings: TemplateStringsArray,
  ...values: RunTemplateValue[]
) => StepRef<O>;

/** An unsafe tagged run template that deliberately interpolates source text. */
export type UnsafeRunTemplate<O extends string = never> = (
  strings: TemplateStringsArray,
  ...values: unknown[]
) => StepRef<O>;

/** The callable surface of {@link run}, including its explicit unsafe escape hatch. */
export interface RunFunction {
  /** The existing command-and-options form. */
  <const O extends string = never>(command: string, opts?: RunStepOpts<O>): StepRef<O>;
  /** Safely interpolates values through step environment variables. */
  <const O extends string = never>(
    strings: TemplateStringsArray,
    ...values: RunTemplateValue[]
  ): StepRef<O>;
  /** Supplies options to a subsequent safe tagged template. */
  <const O extends string = never>(opts: RunStepOpts<O>): RunTemplate<O>;
  /** Deliberately interpolates values directly into shell source. */
  readonly unsafe: UnsafeRunFunction;
}

/** The explicit direct-interpolation escape hatch attached to {@link run}. */
export interface UnsafeRunFunction {
  <const O extends string = never>(command: string, opts?: RunStepOpts<O>): StepRef<O>;
  <const O extends string = never>(
    strings: TemplateStringsArray,
    ...values: unknown[]
  ): StepRef<O>;
  <const O extends string = never>(opts: RunStepOpts<O>): UnsafeRunTemplate<O>;
}

type ShellQuote = "unquoted" | "single" | "double";

interface ShellTemplateState {
  quote: ShellQuote;
  escaped: boolean;
  comment: boolean;
  backtick: boolean;
  commandSubstitutionDepth: number;
  parameterExpansionDepth: number;
}

function newShellTemplateState(): ShellTemplateState {
  return {
    quote: "unquoted",
    escaped: false,
    comment: false,
    backtick: false,
    commandSubstitutionDepth: 0,
    parameterExpansionDepth: 0,
  };
}

/**
 * Scans literal template text just far enough to identify contexts in which
 * inserting a variable expansion would be ambiguous or unsafe. The scanner is
 * intentionally conservative: shell syntax it cannot classify is rejected
 * instead of receiving an unverified escaping claim.
 */
function scanShellTemplateLiteral(source: string, state: ShellTemplateState): void {
  for (let index = 0; index < source.length; index += 1) {
    const char = source[index] ?? "";

    if (state.comment) {
      if (char === "\n" || char === "\r") {
        state.comment = false;
      }
      continue;
    }

    if (state.escaped) {
      state.escaped = false;
      continue;
    }

    if (char === "\\") {
      // A backslash in a single-quoted shell string is literal.
      if (state.quote !== "single") {
        state.escaped = true;
      }
      continue;
    }

    if (state.commandSubstitutionDepth > 0) {
      if (char === "$" && source[index + 1] === "(") {
        state.commandSubstitutionDepth += 1;
        index += 1;
      } else if (char === "(") {
        state.commandSubstitutionDepth += 1;
      } else if (char === ")") {
        state.commandSubstitutionDepth -= 1;
      }
      continue;
    }

    if (state.parameterExpansionDepth > 0) {
      if (char === "$" && source[index + 1] === "{") {
        state.parameterExpansionDepth += 1;
        index += 1;
      } else if (char === "{") {
        state.parameterExpansionDepth += 1;
      } else if (char === "}") {
        state.parameterExpansionDepth -= 1;
      }
      continue;
    }

    if (state.backtick) {
      if (char === "`") {
        state.backtick = false;
      }
      continue;
    }

    if (state.quote === "single") {
      if (char === "'") {
        state.quote = "unquoted";
      }
      continue;
    }

    if (state.quote === "double") {
      if (char === '"') {
        state.quote = "unquoted";
        continue;
      }
    } else if (char === "'" || char === '"') {
      state.quote = char === "'" ? "single" : "double";
      continue;
    }

    if (char === "$" && source[index + 1] === "(") {
      state.commandSubstitutionDepth = 1;
      index += 1;
      continue;
    }
    if (char === "$" && source[index + 1] === "{") {
      state.parameterExpansionDepth = 1;
      index += 1;
      continue;
    }
    if (char === "`") {
      state.backtick = true;
      continue;
    }
    if (
      state.quote === "unquoted" &&
      char === "#" &&
      (index === 0 || /[\s;|&()<>]/.test(source[index - 1] ?? ""))
    ) {
      state.comment = true;
    }
  }
}

function invalidRunTemplate(message: string): LooptError {
  return new LooptError("ambiguous-shell-context", message);
}

function assertSupportedSafeShell(shell: string | undefined): void {
  if (shell === undefined) {
    // The runner's default shell is accepted for compatibility with run`...`.
    // An explicit shell still has to identify a shell whose quoting rules this
    // helper understands.
    return;
  }
  const trimmed = shell.trim();
  const executable = trimmed.split(/\s+/, 1)[0] ?? "";
  const basename = executable.slice(executable.lastIndexOf("/") + 1);
  if (basename !== "bash" && basename !== "sh") {
    throw new LooptError(
      "unsupported-shell",
      "safe run templates support only bash and sh shells; received " +
        JSON.stringify(shell),
    );
  }
}

function assertSafeTemplateInsertion(
  state: ShellTemplateState,
  literal: string,
): void {
  if (state.escaped) {
    throw invalidRunTemplate(
      "cannot interpolate a value after an escape character in a safe run template",
    );
  }
  if (state.comment) {
    throw invalidRunTemplate(
      "cannot interpolate a value inside a shell comment in a safe run template",
    );
  }
  if (state.quote === "single") {
    throw invalidRunTemplate(
      "cannot interpolate a value inside a single-quoted shell string; use double quotes or run.unsafe",
    );
  }
  if (state.backtick) {
    throw invalidRunTemplate(
      "cannot interpolate a value inside backtick command substitution",
    );
  }
  if (state.commandSubstitutionDepth > 0) {
    throw invalidRunTemplate(
      "cannot interpolate a value inside command substitution in a safe run template",
    );
  }
  if (state.parameterExpansionDepth > 0) {
    throw invalidRunTemplate(
      "cannot interpolate a value inside shell parameter expansion in a safe run template",
    );
  }
  if (literal.endsWith("$")) {
    throw invalidRunTemplate(
      "cannot interpolate a value immediately after a shell dollar sign",
    );
  }
}

function assertCompleteSafeTemplate(state: ShellTemplateState): void {
  if (state.escaped) {
    throw invalidRunTemplate("safe run template ends with an escape character");
  }
  if (state.quote !== "unquoted") {
    throw invalidRunTemplate("safe run template contains an unterminated shell quote");
  }
  if (state.backtick) {
    throw invalidRunTemplate("safe run template contains an unterminated backtick substitution");
  }
  if (state.commandSubstitutionDepth > 0) {
    throw invalidRunTemplate(
      "safe run template contains an unterminated command substitution",
    );
  }
  if (state.parameterExpansionDepth > 0) {
    throw invalidRunTemplate(
      "safe run template contains an unterminated shell parameter expansion",
    );
  }
}

function safeTemplateValue(value: unknown): RunTemplateValue {
  if (isExpr(value)) {
    return value;
  }
  if (typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  throw new LooptError(
    "invalid-run-interpolation",
    "safe run template values must be finite numbers, strings, booleans, or expressions",
  );
}

function nextRunEnvironmentName(
  existing: Set<string>,
  ordinal: number,
): string {
  let suffix = ordinal;
  let name = "__LOOPT_RUN_" + suffix;
  while (existing.has(name)) {
    suffix += 1;
    name = "__LOOPT_RUN_" + suffix;
  }
  existing.add(name);
  return name;
}

function buildSafeTemplate(
  strings: TemplateStringsArray,
  values: readonly unknown[],
  opts: RunStepOpts | undefined,
): StepRef {
  if (strings.length !== values.length + 1) {
    throw new LooptError(
      "invalid-run-template",
      "run tagged templates require one more literal fragment than interpolated values",
    );
  }
  assertSupportedSafeShell(opts?.shell);

  const state = newShellTemplateState();
  const env: Record<string, string | number | boolean | Expr> = {};
  const existing = new Set<string>();
  if (opts?.env !== undefined) {
    for (const [key, value] of Object.entries(opts.env)) {
      env[key] = value;
      existing.add(key);
    }
  }

  let command = "";
  for (let index = 0; index < values.length; index += 1) {
    const literal = strings[index] ?? "";
    command += literal;
    scanShellTemplateLiteral(literal, state);
    assertSafeTemplateInsertion(state, literal);

    const name = nextRunEnvironmentName(existing, index);
    env[name] = safeTemplateValue(values[index]);
    command += state.quote === "double" ? "${" + name + "}" : '"${' + name + '}"';
  }
  const tail = strings[strings.length - 1] ?? "";
  command += tail;
  scanShellTemplateLiteral(tail, state);
  assertCompleteSafeTemplate(state);

  if (values.length === 0) {
    return buildRun(command, opts);
  }
  const safeOpts: RunStepOpts = opts === undefined ? { env } : { ...opts, env };
  return buildRun(command, safeOpts);
}

function buildUnsafeTemplate(
  strings: TemplateStringsArray,
  values: readonly unknown[],
  opts: RunStepOpts | undefined,
): StepRef {
  if (strings.length !== values.length + 1) {
    throw new LooptError(
      "invalid-run-template",
      "run tagged templates require one more literal fragment than interpolated values",
    );
  }
  let command = strings[0] ?? "";
  for (let index = 0; index < values.length; index += 1) {
    command += String(values[index]);
    command += strings[index + 1] ?? "";
  }
  return buildRun(command, opts);
}

function isTemplateStrings(value: unknown): value is TemplateStringsArray {
  return (
    Array.isArray(value) &&
    value.every((part: unknown) => typeof part === "string")
  );
}

function isRunOptions(value: unknown): value is RunStepOpts {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function dispatchRun(
  safe: boolean,
  first: unknown,
  rest: readonly unknown[],
): unknown {
  if (typeof first === "string") {
    if (rest.length > 1) {
      throw new LooptError("invalid-run", "run accepts at most one options argument");
    }
    return buildRun(first, rest[0] as RunStepOpts | undefined);
  }
  if (isTemplateStrings(first)) {
    return safe
      ? buildSafeTemplate(first, rest, undefined)
      : buildUnsafeTemplate(first, rest, undefined);
  }
  if (isRunOptions(first) && rest.length === 0) {
    return (strings: TemplateStringsArray, ...values: unknown[]) =>
      safe
        ? buildSafeTemplate(strings, values, first)
        : buildUnsafeTemplate(strings, values, first);
  }
  throw new LooptError(
    "invalid-run",
    "run expects a command string, a tagged template, or tagged-template options",
  );
}

function buildRun<O extends string = never>(
  command: string,
  opts?: RunStepOpts<O>,
): StepRef<O> {
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
  return stepRef("run", entries, opts?.outputs ?? []);
}

const runBuilder = Object.assign(
  function run(first: unknown, ...rest: unknown[]): unknown {
    return dispatchRun(true, first, rest);
  },
  {
    unsafe: function unsafeRun(first: unknown, ...rest: unknown[]): unknown {
      return dispatchRun(false, first, rest);
    },
  },
);

/** Builds a shell command, or safely interpolates a tagged template. */
export const run = runBuilder as RunFunction;

/**
 * Builds a step that runs an action.
 *
 * Keys are emitted in the order `name`, `id`, `if`, `uses`, `with`, `env`,
 * `continueOnError`, `timeoutMinutes`, `background`, `extra`; `with` is
 * omitted when `inputs` is absent or empty.
 */
export function uses<const O extends string = string>(
  action: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts<O>,
): StepRef<O> {
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
  return stepRef("uses", entries, opts?.outputs);
}

/** Builds a step that runs a Docker image as an action (`docker://<image>`). */
export function docker<const O extends string = string>(
  image: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts<O>,
): StepRef<O> {
  return uses<O>("docker://" + image, inputs, opts);
}

/**
 * Builds a step that runs an action from the local repository.
 *
 * @throws {LooptError} `"invalid-local-path"` when `path` does not start with
 * `"./"`.
 */
export function local<const O extends string = string>(
  path: string,
  inputs?: Record<string, string | number | boolean | Expr>,
  opts?: StepOpts<O>,
): StepRef<O> {
  if (!path.startsWith("./")) {
    throw new LooptError(
      "invalid-local-path",
      "local() expects a repository-relative action path starting with \"./\", received " +
        JSON.stringify(path),
    );
  }
  return uses<O>(path, inputs, opts);
}

/**
 * Builds a step that waits for background steps identified by `targets`.
 *
 * A single target lowers to one id; several targets lower to an array.
 */
export function wait(...targets: Array<StepRef | string>): StepRef {
  const value: unknown = targets.length === 1 ? targets[0] : targets;
  return new StepRef<string>("wait", { wait: value }, []);
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
  return stepRef<string>("wait-all", entries, []);
}

/** Builds a step that cancels a background step. */
export function cancel(target: StepRef | string): StepRef {
  return new StepRef<string>("cancel", { cancel: target }, []);
}

/** Builds a step that runs `steps` concurrently and waits for all of them. */
export function parallel(...steps: StepRef[]): StepRef {
  return new StepRef<string>("parallel", { parallel: steps }, []);
}
