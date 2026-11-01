// Contract tests for the schema-driven document normalizer.
//
// Node executes this file directly through its built-in type stripping, so
// only erasable TypeScript syntax is allowed here. Run the suite from the
// repository root with:
//
//     node --test "sdk/typescript/test/normalize.test.ts"

import assert from "node:assert/strict";
import test from "node:test";

import {
  LooptError,
  OrderedObject,
  RawNumber,
  ordered,
  raw,
  stringify,
} from "../src/kit/index.ts";
import type { DocValue } from "../src/kit/index.ts";
import {
  context,
  failure,
  fromJSON,
  github,
  success,
} from "../src/expr/index.ts";
import {
  normalize,
  normalizeWorkflow,
} from "../src/core/normalize.ts";

/** Normalizes `value` against a workflow definition and renders it as compact JSON. */
function render(definition: string, value: DocValue, path?: string): string {
  return stringify(normalize("workflow", definition, value, path));
}

/** Asserts that `action` throws a `LooptError` with exactly `code`, returning it. */
function expectError(code: string, action: () => unknown): LooptError {
  let captured: unknown;
  assert.throws(action, (error: unknown) => {
    captured = error;
    assert.ok(
      error instanceof LooptError,
      `expected a LooptError with code ${code}, got ${String(error)}`,
    );
    assert.equal(error.code, code);
    return true;
  });
  assert.ok(captured instanceof LooptError);
  return captured;
}

test("job-factory maps camelCase properties to exact YAML keys", () => {
  const document = normalize(
    "workflow",
    "job-factory",
    {
      runsOn: "ubuntu-latest",
      timeoutMinutes: 30,
      continueOnError: true,
    },
    "/jobs/build",
  );
  assert.equal(
    stringify(document),
    '{"runs-on":"ubuntu-latest","timeout-minutes":30,"continue-on-error":true}',
  );
});

test("job-factory accepts exact YAML keys as written", () => {
  const document = normalize(
    "workflow",
    "job-factory",
    {
      "runs-on": "ubuntu-latest",
      "timeout-minutes": 30,
      "continue-on-error": true,
    },
    "/jobs/build",
  );
  assert.equal(
    stringify(document),
    '{"runs-on":"ubuntu-latest","timeout-minutes":30,"continue-on-error":true}',
  );
});

test("job-factory rejects an alias and its exact key together", () => {
  expectError("duplicate-key", () =>
    normalize(
      "workflow",
      "job-factory",
      {
        runsOn: "ubuntu-latest",
        "runs-on": "windows-latest",
      },
      "/jobs/build",
    ),
  );
});

test("job-factory rejects unknown keys and reports the path", () => {
  const error = expectError("unknown-key", () =>
    normalize(
      "workflow",
      "job-factory",
      { runsOn: "ubuntu-latest", bogus: 1 },
      "/jobs/build",
    ),
  );
  assert.match(error.message, /bogus/);
  assert.match(error.message, / \(at \/jobs\/build\)$/);
});

test("job-factory reports a missing required property", () => {
  expectError("missing-required", () =>
    normalize("workflow", "job-factory", { steps: [] }, "/jobs/build"),
  );
});

test("unknown schema definitions are rejected", () => {
  expectError("unknown-definition", () =>
    normalize("workflow", "no-such-definition", {}, "/"),
  );
});

test("normalization preserves authored key order", () => {
  assert.equal(
    render(
      "job-factory",
      {
        continueOnError: false,
        runsOn: "ubuntu-latest",
        timeoutMinutes: 1,
        name: "Build",
      },
      "/jobs/build",
    ),
    '{"continue-on-error":false,"runs-on":"ubuntu-latest","timeout-minutes":1,"name":"Build"}',
  );
});

test("extra is appended last with exact names and rendered expressions", () => {
  assert.equal(
    render(
      "job-factory",
      {
        extra: { timeoutMinutes: 5, ref: github.ref },
        runsOn: "ubuntu-latest",
      },
      "/jobs/build",
    ),
    '{"runs-on":"ubuntu-latest","timeoutMinutes":5,"ref":"${{ github.ref }}"}',
  );
});

test("extra keys that repeat a resolved key are rejected", () => {
  expectError("extra-conflict", () =>
    normalize(
      "workflow",
      "job-factory",
      {
        runsOn: "ubuntu-latest",
        extra: { "runs-on": "windows-latest" },
      },
      "/jobs/build",
    ),
  );
});

test("an expression in a string position renders with its wrapper", () => {
  assert.equal(
    render(
      "job-factory",
      { name: github.ref, runsOn: "ubuntu-latest" },
      "/jobs/build",
    ),
    '{"name":"${{ github.ref }}","runs-on":"ubuntu-latest"}',
  );
});

test("if positions use the bare expression while literal scalars stay literal", () => {
  assert.equal(
    render(
      "job-factory",
      { if: success(), runsOn: "ubuntu-latest" },
      "/jobs/build",
    ),
    '{"if":"success()","runs-on":"ubuntu-latest"}',
  );
  assert.equal(
    render("run-step", { if: failure(), run: "echo hi" }, "/jobs/build/steps/0"),
    '{"if":"failure()","run":"echo hi"}',
  );
  assert.equal(
    render(
      "job-factory",
      { if: false, runsOn: "ubuntu-latest" },
      "/jobs/build",
    ),
    '{"if":false,"runs-on":"ubuntu-latest"}',
  );
  assert.equal(
    render("run-step", { if: true, run: "echo hi" }, "/jobs/build/steps/0"),
    '{"if":true,"run":"echo hi"}',
  );
  assert.equal(
    render("run-step", { if: 1, run: "echo hi" }, "/jobs/build/steps/0"),
    '{"if":1,"run":"echo hi"}',
  );
});

test("loose with and env values accept scalars and keep loose keys verbatim", () => {
  assert.equal(
    render(
      "regular-step",
      {
        uses: "actions/setup-node@v4",
        with: {
          "node-version": 20,
          cache: true,
          continueOnError: "yes",
          token: github.ref,
        },
        env: { CI: true, PORT: 8080, runsOn: "no" },
      },
      "/jobs/build/steps/0",
    ),
    '{"uses":"actions/setup-node@v4","with":{"node-version":20,"cache":true,"continueOnError":"yes","token":"${{ github.ref }}"},"env":{"CI":true,"PORT":8080,"runsOn":"no"}}',
  );
});

test("on accepts a single event string", () => {
  assert.equal(render("on-strict", "push", "/on"), '"push"');
});

test("on accepts a sequence of event strings", () => {
  assert.equal(
    render("on-strict", ["push", "workflow_dispatch"], "/on"),
    '["push","workflow_dispatch"]',
  );
});

test("on accepts a mapping and aliases camelCase events", () => {
  assert.equal(
    render(
      "on-strict",
      { pullRequest: null, workflowDispatch: null, push: null },
      "/on",
    ),
    '{"pull_request":null,"workflow_dispatch":null,"push":null}',
  );
});

test("permissions accepts the read-all shorthand and an aliased mapping", () => {
  assert.equal(render("permissions", "read-all", "/permissions"), '"read-all"');
  assert.equal(
    render(
      "permissions",
      { idToken: "write", pullRequests: "read", contents: "none" },
      "/permissions",
    ),
    '{"id-token":"write","pull-requests":"read","contents":"none"}',
  );
});

test("concurrency accepts a string and a mapping with cancelInProgress", () => {
  assert.equal(
    render("workflow-concurrency", "ci-main", "/concurrency"),
    '"ci-main"',
  );
  assert.equal(
    render(
      "workflow-concurrency",
      { group: "ci-main", cancelInProgress: true },
      "/concurrency",
    ),
    '{"group":"ci-main","cancel-in-progress":true}',
  );
});

test("strategy aliases fail-fast and max-parallel and renders an expression matrix", () => {
  assert.equal(
    render(
      "strategy",
      {
        failFast: false,
        maxParallel: 2,
        matrix: fromJSON(context("x")),
      },
      "/jobs/build/strategy",
    ),
    '{"fail-fast":false,"max-parallel":2,"matrix":"${{ fromJSON(x) }}"}',
  );
});

test("runs-on accepts a string, a sequence, and a mapping", () => {
  assert.equal(
    render("runs-on", "ubuntu-latest", "/jobs/build/runs-on"),
    '"ubuntu-latest"',
  );
  assert.equal(
    render("runs-on", ["self-hosted", "linux"], "/jobs/build/runs-on"),
    '["self-hosted","linux"]',
  );
  assert.equal(
    render(
      "runs-on",
      { group: "builders", labels: ["x64", "large"] },
      "/jobs/build/runs-on",
    ),
    '{"group":"builders","labels":["x64","large"]}',
  );
});

test("jobs disambiguate reusable-call jobs from normal jobs", () => {
  assert.equal(
    render(
      "jobs",
      {
        reuse: {
          uses: "octo/repo/.github/workflows/ci.yml@v1",
          with: { level: 3 },
          secrets: "inherit",
        },
        build: {
          runsOn: "ubuntu-latest",
          steps: [{ run: "echo hi" }],
        },
      },
      "/jobs",
    ),
    '{"reuse":{"uses":"octo/repo/.github/workflows/ci.yml@v1","with":{"level":3},"secrets":"inherit"},"build":{"runs-on":"ubuntu-latest","steps":[{"run":"echo hi"}]}}',
  );
});

test("steps disambiguate run, uses, and parallel shapes", () => {
  assert.equal(
    render(
      "steps",
      [
        { run: "echo hi" },
        { uses: "actions/checkout@v4", with: { fetchDepth: 1 } },
        {
          parallel: [
            { run: "npm test" },
            { uses: "actions/cache@v4" },
          ],
        },
      ],
      "/jobs/build/steps",
    ),
    '[{"run":"echo hi"},{"uses":"actions/checkout@v4","with":{"fetchDepth":1}},{"parallel":[{"run":"npm test"},{"uses":"actions/cache@v4"}]}]',
  );
});

test("primitive shape mismatches are rejected", () => {
  expectError("type-mismatch", () =>
    normalize(
      "workflow",
      "job-factory",
      { runsOn: "ubuntu-latest", timeoutMinutes: "thirty" },
      "/jobs/build",
    ),
  );
  expectError("type-mismatch", () =>
    normalize(
      "workflow",
      "job-factory",
      { runsOn: "ubuntu-latest", steps: "echo hi" },
      "/jobs/build",
    ),
  );
});

test("an ambiguous job falls back to the closest job shape", () => {
  const error = expectError("unknown-key", () =>
    normalize(
      "workflow",
      "job",
      {
        runsOn: "ubuntu-latest",
        uses: "octo/repo/.github/workflows/ci.yml@v1",
      },
      "/jobs/build",
    ),
  );
  assert.match(error.message, /uses/);
  assert.match(error.message, / \(at \/jobs\/build\)$/);
});

test("a misspelled job property resolves through jobs and reports unknown-key", () => {
  const error = expectError("unknown-key", () =>
    normalizeWorkflow({
      on: "push",
      jobs: {
        build: { runson: "x", steps: [] },
      },
    }),
  );
  assert.match(error.message, /runson/);
  assert.match(error.message, / \(at \/jobs\/build\)$/);
});

test("a job without runs-on reports the missing required property", () => {
  const error = expectError("missing-required", () =>
    normalize("workflow", "job", { steps: [] }, "/jobs/build"),
  );
  assert.match(error.message, /runs-on/);
});

test("needs rejects an object because it has no mapping shape", () => {
  expectError("no-matching-shape", () =>
    normalize(
      "workflow",
      "needs",
      { build: true },
      "/jobs/build/needs",
    ),
  );
});

test("integer-like matrix include keys survive in authored order", () => {
  assert.equal(
    render(
      "strategy",
      { matrix: { include: [ordered([["2", "two"], ["1", "one"]])] } },
      "/jobs/build/strategy",
    ),
    '{"matrix":{"include":[{"2":"two","1":"one"}]}}',
  );
});

test("RawNumber and OrderedObject values pass through", () => {
  const job = normalize(
    "workflow",
    "job-factory",
    {
      runsOn: "ubuntu-latest",
      timeoutMinutes: raw("9007199254740993"),
    },
    "/jobs/build",
  );
  assert.equal(
    stringify(job),
    '{"runs-on":"ubuntu-latest","timeout-minutes":9007199254740993}',
  );
  assert.ok(typeof job === "object" && job !== null && "timeout-minutes" in job);
  assert.ok(job["timeout-minutes"] instanceof RawNumber);

  const entry = ordered([["2", "two"], ["1", "one"]]);
  const item = normalize(
    "workflow",
    "matrix-filter-item",
    entry,
    "/jobs/build/strategy/matrix/include/0",
  );
  assert.ok(item instanceof OrderedObject);
  assert.equal(stringify(item), '{"2":"two","1":"one"}');
});

test("normalizeWorkflow normalizes through the strict workflow root", () => {
  const root = {
    on: "push",
    permissions: "read-all",
    jobs: {
      build: {
        runsOn: "ubuntu-latest",
        steps: [{ run: "echo hi" }],
      },
    },
  };
  const document = normalizeWorkflow(root);
  assert.deepEqual(
    document,
    normalize("workflow", "workflow-root-strict", root, ""),
  );
  assert.equal(
    stringify(document),
    '{"on":"push","permissions":"read-all","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"echo hi"}]}}}',
  );
});
