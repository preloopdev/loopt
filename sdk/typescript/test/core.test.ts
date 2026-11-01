// Behavioural tests for the schema-driven normalizer and the workflow, job,
// and step constructors. Everything is exercised through the public entry
// point and asserted as exact JSON text.
//
// Node executes this file directly through its built-in type stripping, so
// only erasable TypeScript syntax is allowed here. Run the suite from the
// repository root with:
//
//     node --test "sdk/typescript/test/core.test.ts"

import assert from "node:assert/strict";
import test from "node:test";

import {
  JobRef,
  LooptError,
  PASSES,
  StepRef,
  assemble,
  callWorkflow,
  cancel,
  docker,
  emit,
  eq,
  fromJSON,
  github,
  input,
  job,
  local,
  matrix,
  matrixDefinition,
  parallel,
  run,
  stringify,
  uses,
  wait,
  waitAll,
  workflow,
} from "../src/index.ts";
import type { DocValue } from "../src/index.ts";

/** Assert that `action` throws a `LooptError` carrying exactly `code`. */
function throwsCode(code: string, action: () => unknown): void {
  assert.throws(action, (error: unknown) => {
    assert.ok(
      error instanceof LooptError,
      `expected a LooptError with code ${code}, got ${String(error)}`,
    );
    assert.equal(error.code, code);
    return true;
  });
}

/** Run `action`, requiring it to throw, and return the resulting error. */
function captureError(action: () => unknown): LooptError {
  try {
    action();
  } catch (error) {
    assert.ok(error instanceof LooptError, `expected a LooptError, got ${String(error)}`);
    return error;
  }
  assert.fail("expected the action to throw a LooptError");
}

/** Serialize a documentable through the public `toDocument()` method. */
function documentJson(target: { toDocument(): DocValue }): string {
  return stringify(target.toDocument());
}

/** Put `steps` inside a minimal build job and serialize the whole workflow. */
function stepsJson(steps: Array<StepRef | StepRef[]>): string {
  return documentJson(
    workflow(undefined, {
      on: "push",
      jobs: {
        build: job({ runsOn: "ubuntu-latest", steps }),
      },
    }),
  );
}

test("the canonical workflow lowers byte for byte", () => {
  const w = workflow("ci", {
    on: { push: { branches: ["main"] }, pullRequest: null },
    permissions: { contents: "read" },
    jobs: {
      test: job({
        runsOn: "ubuntu-latest",
        timeoutMinutes: 30,
        steps: [
          uses("actions/checkout@abc", { "persist-credentials": false }),
          run("npm test", { name: "Test" }),
        ],
      }),
    },
  });

  assert.equal(
    documentJson(w),
    '{"name":"ci","on":{"push":{"branches":["main"]},"pull_request":null},"permissions":{"contents":"read"},"jobs":{"test":{"runs-on":"ubuntu-latest","timeout-minutes":30,"steps":[{"uses":"actions/checkout@abc","with":{"persist-credentials":false}},{"name":"Test","run":"npm test"}]}}}',
  );
});

test("an undefined workflow name is omitted", () => {
  const w = workflow(undefined, {
    on: "push",
    jobs: {
      build: job({ runsOn: "ubuntu-latest", steps: [run("true")] }),
    },
  });

  assert.equal(
    documentJson(w),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"true"}]}}}',
  );
});

test("run steps use the canonical key order for every option", () => {
  const step = run("echo hi", {
    name: "Greet",
    id: "greet",
    if: eq(github.ref, "refs/heads/main"),
    env: { A: "1", B: 2, C: true, D: github.sha },
    shell: "bash",
    workingDirectory: "src",
    continueOnError: true,
    timeoutMinutes: 5,
    background: true,
    extra: { "x-run": "yes" },
  });

  assert.equal(
    stepsJson([step]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"name":"Greet","id":"greet","if":"github.ref == \'refs/heads/main\'","run":"echo hi","env":{"A":"1","B":2,"C":true,"D":"${{ github.sha }}"},"shell":"bash","working-directory":"src","continue-on-error":true,"timeout-minutes":5,"background":true,"x-run":"yes"}]}}}',
  );
});

test("uses steps use the canonical key order for every option", () => {
  const step = uses(
    "actions/cache@v4",
    { path: "~/.npm", key: "lock" },
    {
      name: "Cache",
      id: "cache",
      if: eq(github.ref, "refs/heads/main"),
      env: { A: 1 },
      continueOnError: false,
      timeoutMinutes: 2,
      background: true,
      extra: { "x-uses": 3 },
    },
  );

  assert.equal(
    stepsJson([step]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"name":"Cache","id":"cache","if":"github.ref == \'refs/heads/main\'","uses":"actions/cache@v4","with":{"path":"~/.npm","key":"lock"},"env":{"A":1},"continue-on-error":false,"timeout-minutes":2,"background":true,"x-uses":3}]}}}',
  );
});

test("uses steps omit with when inputs are absent or empty", () => {
  const document = stepsJson([
    uses("actions/checkout@v4"),
    uses("actions/checkout@v4", {}),
  ]);

  assert.equal(
    document,
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"uses":"actions/checkout@v4"},{"uses":"actions/checkout@v4"}]}}}',
  );
});

test("docker prefixes the image with docker:// and keeps inputs", () => {
  assert.equal(
    stepsJson([
      docker("alpine:3.20"),
      docker("alpine:3.20", { cmd: "ls" }),
    ]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"uses":"docker://alpine:3.20"},{"uses":"docker://alpine:3.20","with":{"cmd":"ls"}}]}}}',
  );
});

test("local uses the path relative to the repository and rejects bare paths", () => {
  assert.equal(
    stepsJson([
      local("./actions/hello"),
      local("./actions/hello", { who: "world" }),
    ]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"uses":"./actions/hello"},{"uses":"./actions/hello","with":{"who":"world"}}]}}}',
  );

  throwsCode("invalid-local-path", () => local("actions/hello"));
});

test("wait, cancel, parallel, and waitAll lower to their schema shapes", () => {
  const first = run("one", { id: "first", background: true });
  const second = run("two", { id: "second", background: true });

  assert.equal(
    stepsJson([
      first,
      second,
      wait(first),
      wait(first, second),
      cancel(second),
      waitAll(),
      waitAll({ name: "All", id: "all", continueOnError: true }),
    ]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"id":"first","run":"one","background":true},{"id":"second","run":"two","background":true},{"wait":"first"},{"wait":["first","second"]},{"cancel":"second"},{"wait-all":null},{"name":"All","id":"all","continue-on-error":true,"wait-all":null}]}}}',
  );
});

test("wait and cancel reject targets without an explicit step id", () => {
  const orphanWait = run("orphan");
  throwsCode("missing-step-id", () =>
    stepsJson([orphanWait, wait(orphanWait)]),
  );

  const orphanCancel = run("orphan");
  throwsCode("missing-step-id", () =>
    stepsJson([orphanCancel, cancel(orphanCancel)]),
  );
});

test("parallel nests fully lowered steps", () => {
  assert.equal(
    stepsJson([
      parallel(
        run("a"),
        run("b", { id: "b" }),
        uses("actions/x@v1", { k: "v" }),
      ),
    ]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"parallel":[{"run":"a"},{"id":"b","run":"b"},{"uses":"actions/x@v1","with":{"k":"v"}}]}]}}}',
  );
});

test("nested step arrays flatten in declaration order", () => {
  assert.equal(
    stepsJson([
      [run("a"), run("b")],
      [run("c", { id: "c" })],
    ]),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"a"},{"run":"b"},{"id":"c","run":"c"}]}}}',
  );
});

test("a job placed under two ids is rejected", () => {
  const build = job({ runsOn: "ubuntu-latest", steps: [run("true")] });
  const w = workflow("twice", { on: "push", jobs: { a: build, b: build } });

  throwsCode("job-placed-twice", () => w.toDocument());
});

test("a non-JobRef in jobs is rejected", () => {
  const jobs = { broken: null } as unknown as Record<string, JobRef>;
  const w = workflow("invalid", { on: "push", jobs });

  throwsCode("invalid-job", () => w.toDocument());
});

test("a non-StepRef in steps is rejected", () => {
  const steps = [null] as unknown as StepRef[];

  throwsCode("invalid-step", () =>
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { build: job({ runsOn: "ubuntu-latest", steps }) },
      }),
    ),
  );
});

test("the same step used by two jobs is rejected", () => {
  const shared = run("shared", { id: "shared" });
  const first = job({ runsOn: "ubuntu-latest", steps: [shared] });
  const second = job({ runsOn: "ubuntu-latest", steps: [shared] });

  throwsCode("step-reused", () =>
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { first, second },
      }),
    ),
  );
});

test("a duplicate explicit step id inside one job is rejected", () => {
  const build = job({
    runsOn: "ubuntu-latest",
    steps: [run("a", { id: "same" }), run("b", { id: "same" })],
  });

  throwsCode("duplicate-step-id", () =>
    documentJson(
      workflow(undefined, { on: "push", jobs: { build } }),
    ),
  );
});

test("needs accepts a JobRef and lowers to the placed job id", () => {
  const build = job({ runsOn: "ubuntu-latest", steps: [run("build")] });
  const testJob = job({
    needs: build,
    runsOn: "ubuntu-latest",
    steps: [run("test")],
  });

  assert.equal(
    documentJson(
      workflow("needs", {
        on: "push",
        jobs: { build, test: testJob },
      }),
    ),
    '{"name":"needs","on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"build"}]},"test":{"needs":"build","runs-on":"ubuntu-latest","steps":[{"run":"test"}]}}}',
  );
});

test("needs accepts a string and an array mixing strings and JobRefs", () => {
  const fromString = job({
    needs: "build",
    runsOn: "ubuntu-latest",
    steps: [run("test")],
  });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: {
          build: job({ runsOn: "ubuntu-latest", steps: [run("build")] }),
          test: fromString,
        },
      }),
    ),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"build"}]},"test":{"needs":"build","runs-on":"ubuntu-latest","steps":[{"run":"test"}]}}}',
  );

  const a = job({ runsOn: "ubuntu-latest", steps: [run("a")] });
  const b = job({ runsOn: "ubuntu-latest", steps: [run("b")] });
  const c = job({
    needs: [a, "a", b, "b"],
    runsOn: "ubuntu-latest",
    steps: [run("c")],
  });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { a, b, c },
      }),
    ),
    '{"on":"push","jobs":{"a":{"runs-on":"ubuntu-latest","steps":[{"run":"a"}]},"b":{"runs-on":"ubuntu-latest","steps":[{"run":"b"}]},"c":{"needs":["a","b"],"runs-on":"ubuntu-latest","steps":[{"run":"c"}]}}}',
  );
});

test("a needs reference to a job from another workflow is rejected", () => {
  const foreign = job({ runsOn: "ubuntu-latest", steps: [run("foreign")] });
  const build = job({
    needs: foreign,
    runsOn: "ubuntu-latest",
    steps: [run("build")],
  });

  throwsCode("unknown-job-reference", () =>
    documentJson(
      workflow(undefined, { on: "push", jobs: { build } }),
    ),
  );
});

test("JobRef exposes its id only after the workflow is assembled", () => {
  const build = job({ runsOn: "ubuntu-latest", steps: [run("true")] });

  throwsCode("unplaced-job", () => build.id);

  const w = workflow("placed", { on: "push", jobs: { build } });
  assert.equal(
    documentJson(w),
    '{"name":"placed","on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"true"}]}}}',
  );
  assert.equal(build.id, "build");
});

test("assembling twice is stable and leaves the authoring objects untouched", () => {
  const buildSteps = [run("build")];
  const build = job({ runsOn: "ubuntu-latest", steps: buildSteps });

  const strategy = { matrix: matrix({ os: ["a"] }) };
  const testSteps: Array<StepRef | StepRef[]> = [
    [run("unit", { id: "unit", env: { TOKEN: github.token } })],
    [run("lint")],
  ];
  const testJob = job({
    needs: build,
    runsOn: "ubuntu-latest",
    strategy,
    steps: testSteps,
  });

  const definition = { on: "push" as const, jobs: { build, test: testJob } };
  const w = workflow("repeat", definition);

  const firstStep = buildSteps[0];
  assert.ok(firstStep instanceof StepRef);
  const stepBefore = stringify(firstStep.def as unknown as DocValue);
  const matrixBefore = stringify(matrixDefinition(strategy.matrix));

  const first = documentJson(w);
  const second = documentJson(w);

  assert.equal(first, second);
  assert.deepEqual(Object.keys(definition), ["on", "jobs"]);
  assert.equal(definition.jobs.build, build);
  assert.equal(definition.jobs.test, testJob);
  assert.equal(build.placedId, "build");
  assert.equal(testJob.placedId, "test");
  assert.equal(build.def.steps, buildSteps);
  assert.equal(testJob.def.steps, testSteps);
  assert.equal(testJob.def.needs, build);
  assert.equal(testJob.def.strategy, strategy);
  assert.equal(matrixBefore, stringify(matrixDefinition(strategy.matrix)));
  assert.equal(stringify(firstStep.def as unknown as DocValue), stepBefore);
  assert.equal(firstStep.def.run, "build");
  assert.deepEqual(Object.keys(firstStep.def), ["run"]);
});

test("callWorkflow lowers uses first and keeps with and secrets", () => {
  const called = callWorkflow("./.github/workflows/x.yml", {
    with: { a: 1 },
    secrets: "inherit",
  });

  assert.equal(
    documentJson(
      workflow("caller", {
        on: "push",
        jobs: { reuse: called },
      }),
    ),
    '{"name":"caller","on":"push","jobs":{"reuse":{"uses":"./.github/workflows/x.yml","with":{"a":1},"secrets":"inherit"}}}',
  );
});

test("matrix lowers to a plain matrix object and exposes axis expressions", () => {
  const m = matrix(
    { os: ["a", "b"], node: [20, 22] },
    { include: [{ os: "c", node: 24 }] },
  );
  const setup = uses("actions/setup-node@v1", { version: m.node });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: {
          test: job({
            runsOn: "ubuntu-latest",
            strategy: { matrix: m, failFast: false },
            steps: [setup],
          }),
        },
      }),
    ),
    '{"on":"push","jobs":{"test":{"runs-on":"ubuntu-latest","strategy":{"matrix":{"os":["a","b"],"node":[20,22],"include":[{"os":"c","node":24}]},"fail-fast":false},"steps":[{"uses":"actions/setup-node@v1","with":{"version":"${{ matrix.node }}"}}]}}}',
  );
});

test("matrix.fromJson lowers to its expression while axes stay readable", () => {
  const m = matrix.fromJson<{ service: string }>(fromJSON("vars.matrix"));
  const step = uses("actions/deploy@v1", { service: m.service });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: {
          deploy: job({
            runsOn: "ubuntu-latest",
            strategy: { matrix: m, failFast: false },
            steps: [step],
          }),
        },
      }),
    ),
    '{"on":"push","jobs":{"deploy":{"runs-on":"ubuntu-latest","strategy":{"matrix":"${{ fromJSON(\'vars.matrix\') }}","fail-fast":false},"steps":[{"uses":"actions/deploy@v1","with":{"service":"${{ matrix.service }}"}}]}}}',
  );
});

test("input helpers build dispatch input definitions in canonical order", () => {
  const w = workflow("inputs", {
    on: {
      workflowDispatch: {
        inputs: {
          name: input.string({
            description: "Who",
            required: true,
            default: "world",
          }),
          count: input.number({ default: 3 }),
          dry: input.boolean({ required: false, default: false }),
          mode: input.choice({
            description: "Mode",
            required: true,
            default: "fast",
            options: ["fast", "slow"],
          }),
          target: input.environment({ description: "Target" }),
        },
      },
    },
    jobs: {
      noop: job({ runsOn: "ubuntu-latest", steps: [run("true")] }),
    },
  });

  assert.equal(
    documentJson(w),
    '{"name":"inputs","on":{"workflow_dispatch":{"inputs":{"name":{"description":"Who","type":"string","required":true,"default":"world"},"count":{"type":"number","default":3},"dry":{"type":"boolean","required":false,"default":false},"mode":{"description":"Mode","type":"choice","required":true,"default":"fast","options":["fast","slow"]},"target":{"description":"Target","type":"environment"}}}},"jobs":{"noop":{"runs-on":"ubuntu-latest","steps":[{"run":"true"}]}}}',
  );
});

test("if expressions render bare while value expressions keep the wrapper", () => {
  const condition = eq(github.ref, "refs/heads/main");

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: {
          build: job({
            if: condition,
            runsOn: "ubuntu-latest",
            env: { A: github.sha },
            steps: [
              run("npm test", {
                if: condition,
                env: { A: github.sha },
              }),
            ],
          }),
        },
      }),
    ),
    '{"on":"push","jobs":{"build":{"if":"github.ref == \'refs/heads/main\'","runs-on":"ubuntu-latest","env":{"A":"${{ github.sha }}"},"steps":[{"if":"github.ref == \'refs/heads/main\'","run":"npm test","env":{"A":"${{ github.sha }}"}}]}}}',
  );
});

test("extra entries are appended last with their exact names", () => {
  const w = workflow("extras", {
    on: "push",
    extra: { "workflow-extra": "w", Exact_Key: 1 },
    jobs: {
      build: job({
        extra: { "job-extra": true },
        runsOn: "ubuntu-latest",
        steps: [run("x", { extra: { "step-extra": "s" } })],
      }),
    },
  });

  assert.equal(
    documentJson(w),
    '{"name":"extras","on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"x","step-extra":"s"}],"job-extra":true}},"workflow-extra":"w","Exact_Key":1}',
  );
});

test("an unknown job key is rejected with unknown-key and names the key", () => {
  const build = job({ runson: "x", steps: [run("true")] } as never);
  const w = workflow("w", { on: "push", jobs: { a: build } });

  const error = captureError(() => w.toDocument());
  assert.equal(error.code, "unknown-key");
  assert.match(error.message, /runson/);
  assert.match(error.message, /\(at \/jobs\/a\)$/);
});

test("a job without runs-on is rejected with missing-required", () => {
  const build = job({ steps: [run("true")] } as never);
  const w = workflow("w", { on: "push", jobs: { a: build } });

  const error = captureError(() => w.toDocument());
  assert.equal(error.code, "missing-required");
  assert.match(error.message, /runs-on/);
});

test("emit writes exactly one JSON line through the supplied writer", () => {
  const w = workflow("emitted", {
    on: "push",
    jobs: {
      build: job({ runsOn: "ubuntu-latest", steps: [run("true")] }),
    },
  });

  let written = "";
  emit(w, (text: string) => {
    written += text;
  });

  const expected = documentJson(w);
  assert.equal(written, expected + "\n");
  assert.equal(written.split("\n").length, 2);
});

test("the public entry point exports the core constructors and passes", () => {
  assert.equal(typeof workflow, "function");
  assert.equal(typeof job, "function");
  assert.equal(typeof callWorkflow, "function");
  assert.equal(typeof run, "function");
  assert.equal(typeof uses, "function");
  assert.equal(typeof docker, "function");
  assert.equal(typeof local, "function");
  assert.equal(typeof wait, "function");
  assert.equal(typeof waitAll, "function");
  assert.equal(typeof cancel, "function");
  assert.equal(typeof parallel, "function");
  assert.equal(typeof matrix, "function");
  assert.equal(typeof matrix.fromJson, "function");
  assert.equal(typeof matrixDefinition, "function");
  assert.equal(typeof input, "object");
  assert.equal(typeof input.string, "function");
  assert.equal(typeof input.number, "function");
  assert.equal(typeof input.boolean, "function");
  assert.equal(typeof input.choice, "function");
  assert.equal(typeof input.environment, "function");
  assert.equal(typeof JobRef, "function");
  assert.equal(typeof StepRef, "function");
  assert.equal(typeof assemble, "function");
  assert.ok(Array.isArray(PASSES));
  assert.equal(PASSES.length, 4);
  assert.ok(PASSES.every((pass) => typeof pass === "function"));
});
