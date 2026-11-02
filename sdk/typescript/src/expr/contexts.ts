import { Expr, Precedence } from "../kit/index.ts";
import { operand } from "./literal.ts";

/** Matches property names GitHub accepts in dereference syntax. */
const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_-]*$/;

/**
 * Members of {@link Expr} that keep their real meaning when read from a
 * {@link DynExpr}; every other string property is treated as a context member.
 */
const RESERVED_MEMBERS: ReadonlySet<string> = new Set([
  "source",
  "contexts",
  "precedence",
  "toString",
  "bare",
  "constructor",
  "then",
  "valueOf",
]);

/**
 * A context expression that can be navigated dynamically: any property read
 * yields another {@link DynExpr}, rendered as `base.name` or `base['name']`.
 *
 * Real {@link Expr} members (`source`, `contexts`, `precedence`, `toString`,
 * `bare`, `constructor`, `then`, `valueOf`) and symbols pass through
 * unchanged. Use {@link DynExpr.at} to read a payload field whose name would
 * otherwise collide with an `Expr` member.
 */
export type DynExpr = Expr<unknown> & {
  readonly [member: string]: DynExpr;
} & {
  /**
   * Indexes into this context with `key`, rendering `base[key]`. Unlike a
   * property read, this works for any field name, including `source` or
   * `toString`.
   */
  readonly at: (key: unknown) => DynExpr;
};

/** Escapes `value` as a GitHub Actions single-quoted string literal. */
function quote(value: string): string {
  return "'" + value.replace(/'/g, "''") + "'";
}

/** Builds `base.name` or `base['name']` at Member precedence. */
function member(base: Expr<unknown>, name: string): DynExpr {
  const source = IDENTIFIER.test(name)
    ? base.source + "." + name
    : base.source + "[" + quote(name) + "]";
  return dyn(
    new Expr<unknown>(source, {
      contexts: base.contexts,
      precedence: Precedence.Member,
    }),
  );
}

/** Builds `base[operand(key)]` at Member precedence. */
function memberAt(base: Expr<unknown>, key: unknown): DynExpr {
  const keyExpression = operand(key);
  const contexts = new Set<string>(base.contexts);
  for (const name of keyExpression.contexts) {
    contexts.add(name.toLowerCase());
  }
  return dyn(
    new Expr<unknown>(base.source + "[" + keyExpression.source + "]", {
      contexts,
      precedence: Precedence.Member,
    }),
  );
}

/** A static table mapping camelCase aliases to their exact GitHub keys. */
type AliasTable = Readonly<Record<string, string>>;

/**
 * Wraps `base` in a Proxy that turns arbitrary property reads into member
 * expressions. `aliases` maps camelCase aliases to their exact GitHub keys.
 */
function dyn(base: Expr<unknown>, aliases?: AliasTable): DynExpr {
  const handler: ProxyHandler<Expr<unknown>> = {
    get(target, property, receiver) {
      if (typeof property === "symbol") {
        return Reflect.get(target, property, receiver);
      }
      if (property === "at") {
        return (key: unknown): DynExpr => memberAt(target, key);
      }
      if (RESERVED_MEMBERS.has(property)) {
        return Reflect.get(target, property, receiver);
      }
      const exact = aliases !== undefined && Object.hasOwn(aliases, property) ? aliases[property] : undefined;
      return member(target, exact === undefined ? property : exact);
    },
  };
  return new Proxy(base, handler) as unknown as DynExpr;
}

/**
 * Builds a root context expression named `name`. Its contexts set contains
 * the lowercase form of `name`.
 */
export function context(name: string): DynExpr {
  return dyn(new Expr<unknown>(name, { contexts: [name] }));
}

/** camelCase aliases for the snake_case members of the `github` context. */
const GITHUB_ALIASES: AliasTable = {
  actionPath: "action_path",
  actionRef: "action_ref",
  actionRepository: "action_repository",
  actionStatus: "action_status",
  actorId: "actor_id",
  apiUrl: "api_url",
  artifactsList: "artifacts_list",
  baseRef: "base_ref",
  eventName: "event_name",
  eventPath: "event_path",
  graphqlUrl: "graphql_url",
  headRef: "head_ref",
  refName: "ref_name",
  refProtected: "ref_protected",
  refType: "ref_type",
  repositoryId: "repository_id",
  repositoryOwner: "repository_owner",
  repositoryOwnerId: "repository_owner_id",
  repositoryUrl: "repositoryUrl",
  retentionDays: "retention_days",
  runAttempt: "run_attempt",
  runId: "run_id",
  runNumber: "run_number",
  secretSource: "secret_source",
  serverUrl: "server_url",
  triggeringActor: "triggering_actor",
  workflowRef: "workflow_ref",
  workflowSha: "workflow_sha",
};

/** camelCase aliases for the snake_case members of the `runner` context. */
const RUNNER_ALIASES: AliasTable = {
  toolCache: "tool_cache",
};

/** camelCase aliases for the kebab-case members of the `strategy` context. */
const STRATEGY_ALIASES: AliasTable = {
  failFast: "fail-fast",
  jobIndex: "job-index",
  jobTotal: "job-total",
  maxParallel: "max-parallel",
};

/** camelCase aliases for the snake_case members of the `job` context. */
const JOB_ALIASES: AliasTable = {
  checkRunId: "check_run_id",
  workflowFilePath: "workflow_file_path",
  workflowRef: "workflow_ref",
  workflowRepository: "workflow_repository",
  workflowSha: "workflow_sha",
};

/**
 * The `github` context: information about the workflow run. Snake_case
 * members are reachable through the dynamic index signature and through their
 * camelCase aliases; `event` stays a plain dynamic object.
 */
export type GithubContext = DynExpr & {
  readonly actionPath: Expr<string>;
  readonly actionRef: Expr<string>;
  readonly actionRepository: Expr<string>;
  readonly actionStatus: Expr<string>;
  readonly actorId: Expr<string>;
  readonly apiUrl: Expr<string>;
  readonly artifactsList: Expr<string>;
  readonly baseRef: Expr<string>;
  readonly event: DynExpr;
  readonly eventName: Expr<string>;
  readonly eventPath: Expr<string>;
  readonly graphqlUrl: Expr<string>;
  readonly headRef: Expr<string>;
  readonly refName: Expr<string>;
  readonly refProtected: Expr<boolean>;
  readonly refType: Expr<string>;
  readonly repositoryId: Expr<string>;
  readonly repositoryOwner: Expr<string>;
  readonly repositoryOwnerId: Expr<string>;
  readonly repositoryUrl: Expr<string>;
  readonly retentionDays: Expr<string>;
  readonly runAttempt: Expr<string>;
  readonly runId: Expr<string>;
  readonly runNumber: Expr<string>;
  readonly secretSource: Expr<string>;
  readonly serverUrl: Expr<string>;
  readonly triggeringActor: Expr<string>;
  readonly workflowRef: Expr<string>;
  readonly workflowSha: Expr<string>;
};

/** The `runner` context: information about the runner executing the job. */
export type RunnerContext = DynExpr & {
  readonly toolCache: Expr<string>;
};

/** The `strategy` context: information about the current matrix strategy. */
export type StrategyContext = DynExpr & {
  readonly failFast: Expr<boolean>;
  readonly jobIndex: Expr<number>;
  readonly jobTotal: Expr<number>;
  readonly maxParallel: Expr<number>;
};

/** The `job` context: information about the currently running job. */
export type JobContext = DynExpr & {
  readonly checkRunId: Expr<number>;
  readonly workflowFilePath: Expr<string>;
  readonly workflowRef: Expr<string>;
  readonly workflowRepository: Expr<string>;
  readonly workflowSha: Expr<string>;
};

/** The `github` context: information about the workflow run. */
export const github: GithubContext = dyn(
  new Expr<unknown>("github", { contexts: ["github"] }),
  GITHUB_ALIASES,
) as unknown as GithubContext;

/** The `env` context: variables set in a workflow, job, or step. */
export const env: DynExpr = context("env");

/** The `vars` context: configuration variables set at the repository, organization, or environment level. */
export const vars: DynExpr = context("vars");

/** The `secrets` context: names and values of secrets available to the run. */
export const secrets: DynExpr = context("secrets");

/** The `inputs` context: inputs of a reusable or manually triggered workflow. */
export const inputs: DynExpr = context("inputs");

/** The `runner` context: information about the runner executing the job. */
export const runner: RunnerContext = dyn(
  new Expr<unknown>("runner", { contexts: ["runner"] }),
  RUNNER_ALIASES,
) as unknown as RunnerContext;

/** The `strategy` context: information about the current matrix strategy. */
export const strategy: StrategyContext = dyn(
  new Expr<unknown>("strategy", { contexts: ["strategy"] }),
  STRATEGY_ALIASES,
) as unknown as StrategyContext;

/** The `job` context, named `jobContext` to leave `job` free for a future constructor. */
export const jobContext: JobContext = dyn(
  new Expr<unknown>("job", { contexts: ["job"] }),
  JOB_ALIASES,
) as unknown as JobContext;

/** The `matrix` context: the user-defined axes of the current matrix
 * combination (`${{ matrix.os }}`). Named `matrixContext` to leave `matrix`
 * free for the `strategy.matrix` builder in `core`. No aliases — every
 * field is user-defined. */
export const matrixContext: DynExpr = context("matrix");
