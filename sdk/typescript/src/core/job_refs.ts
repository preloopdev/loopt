import { LooptError } from "../kit/index.ts";
import type { JobRef } from "./job_ref.ts";
import { StepRef } from "./step_ref.ts";
import { collectTokens, tokenKind } from "./ref_tokens.ts";
import type { AssembleContext, Pass } from "./passes.ts";

/** Step-definition keys whose values name other steps. */
const STEP_TARGET_KEYS = ["wait", "cancel"] as const;

/** One placed job as recorded by the assembly passes. */
type JobEntry = AssembleContext["jobs"][number];

/**
 * Visits the definition of `step` and of every step it references through
 * `parallel`, `wait`, and `cancel`, depth-first in document order and at most
 * once per step.
 */
function visitStep(
  step: StepRef,
  seen: Set<StepRef>,
  visit: (def: { [key: string]: unknown }) => void,
): void {
  if (seen.has(step)) {
    return;
  }
  seen.add(step);
  visit(step.def);
  const parallel: unknown = step.def.parallel;
  if (Array.isArray(parallel)) {
    for (const item of parallel) {
      if (item instanceof StepRef) {
        visitStep(item, seen, visit);
      }
    }
  }
  for (const key of STEP_TARGET_KEYS) {
    const target: unknown = step.def[key];
    if (target instanceof StepRef) {
      visitStep(target, seen, visit);
    } else if (Array.isArray(target)) {
      for (const item of target) {
        if (item instanceof StepRef) {
          visitStep(item, seen, visit);
        }
      }
    }
  }
}

/** Appends `owner` unless an equivalent dependency is already listed. */
function addNeed(targets: Array<JobRef | string>, owner: JobRef): void {
  for (const target of targets) {
    if (target === owner) {
      return;
    }
    if (
      typeof target === "string" &&
      owner.placedId !== undefined &&
      target === owner.placedId
    ) {
      return;
    }
  }
  targets.push(owner);
}

/**
 * Derives job dependencies from the job references used in expressions.
 *
 * Every job token found in a job's working definition (except its `needs`
 * key) or in one of its steps' definitions becomes a dependency on the job
 * that owns the token. Explicit `needs` stay first, and derived dependencies
 * keep the order in which they are first seen.
 *
 * @throws {LooptError} `"unknown-job-reference"` when a reference points at a
 * job outside this workflow.
 * @throws {LooptError} `"self-reference"` when a job references itself.
 */
export const deriveNeeds: Pass = (context: AssembleContext): void => {
  const byToken = new Map<string, JobEntry>();
  for (const job of context.jobs) {
    byToken.set(job.ref.token, job);
  }

  for (const job of context.jobs) {
    const addDependency = (owner: JobEntry): void => {
      const targets = context.needs.get(job.ref);
      if (targets === undefined) {
        context.needs.set(job.ref, [owner.ref]);
        return;
      }
      addNeed(targets, owner.ref);
    };

    const consider = (value: unknown): void => {
      for (const token of collectTokens(value)) {
        if (tokenKind(token) !== "job") {
          continue;
        }
        const owner = byToken.get(token);
        if (owner === undefined) {
          throw new LooptError(
            "unknown-job-reference",
            "job " +
              JSON.stringify(job.id) +
              " references a job that is not part of this workflow",
          );
        }
        if (owner.ref === job.ref) {
          throw new LooptError(
            "self-reference",
            "job " +
              JSON.stringify(job.id) +
              " references its own result or outputs",
          );
        }
        addDependency(owner);
      }
    };

    for (const [key, value] of Object.entries(job.def)) {
      if (key === "needs") {
        continue;
      }
      consider(value);
    }

    const seen = new Set<StepRef>();
    for (const step of job.steps) {
      visitStep(step, seen, consider);
    }
  }
};

/**
 * Validates explicit dependency ids and rejects cyclic dependency graphs.
 *
 * Every string in `needs` must name a job of the workflow, and following the
 * final `needs` graph must never return to a job already on the path.
 *
 * @throws {LooptError} `"unknown-job-reference"` when `needs` names a job
 * outside this workflow.
 * @throws {LooptError} `"needs-cycle"` when the dependency graph contains a
 * cycle; the message spells it out as `a -> b -> a`.
 */
export const checkNeeds: Pass = (context: AssembleContext): void => {
  const ids = new Set<string>();
  for (const job of context.jobs) {
    ids.add(job.id);
  }

  const graph = new Map<string, string[]>();
  for (const job of context.jobs) {
    const dependencies: string[] = [];
    const targets = context.needs.get(job.ref) ?? [];
    for (const target of targets) {
      const id = typeof target === "string" ? target : target.id;
      if (!ids.has(id)) {
        throw new LooptError(
          "unknown-job-reference",
          "job " +
            JSON.stringify(job.id) +
            " needs " +
            JSON.stringify(id) +
            ", which is not a job in this workflow",
        );
      }
      if (!dependencies.includes(id)) {
        dependencies.push(id);
      }
    }
    graph.set(job.id, dependencies);
  }

  const state = new Map<string, "visiting" | "done">();
  const stack: string[] = [];

  const visit = (id: string): void => {
    const mark = state.get(id);
    if (mark === "done") {
      return;
    }
    if (mark === "visiting") {
      const cycle = stack.slice(stack.indexOf(id));
      cycle.push(id);
      throw new LooptError(
        "needs-cycle",
        "job dependencies form a cycle: " + cycle.join(" -> "),
      );
    }
    state.set(id, "visiting");
    stack.push(id);
    const dependencies = graph.get(id);
    if (dependencies !== undefined) {
      for (const dependency of dependencies) {
        visit(dependency);
      }
    }
    stack.pop();
    state.set(id, "done");
  };

  for (const job of context.jobs) {
    visit(job.id);
  }
};
