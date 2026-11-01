// Contract tests for job and step references: placeholder tokens, needs
// derived from references, automatic step ids, and the authoring errors
// reported while assembling a workflow. Documents are compared as exact JSON
// text.
//
// Node executes this file directly through its built-in type stripping, so
// only erasable TypeScript syntax is allowed here. Run the suite from the
// repository root with:
//
//     node --test "sdk/typescript/test/refs.test.ts"

import assert from "node:assert/strict";
import test from "node:test";

import {
  Expr,
  LooptError,
  OrderedObject,
  RawNumber,
  StepRef,
  callWorkflow,
  eq,
  job,
  ordered,
  parallel,
  raw,
  run,
  stringify,
  uses,
  workflow,
} from "../src/index.ts";
import type { DocValue } from "../src/index.ts";
import {
  collectTokens,
  newToken,
  substituteTokens,
  tokenKind,
  tokensIn,
} from "../src/core/ref_tokens.ts";

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

/** Serialize a documentable through its public `toDocument()` method. */
function documentJson(target: { toDocument(): DocValue }): string {
  return stringify(target.toDocument());
}

test("newToken builds well-formed, unique tokens", () => {
  const tokens = [newToken("job"), newToken("job"), newToken("step"), newToken("step")];

  assert.match(tokens[0], /^__loopt_job_[0-9]+__$/);
  assert.match(tokens[1], /^__loopt_job_[0-9]+__$/);
  assert.match(tokens[2], /^__loopt_step_[0-9]+__$/);
  assert.match(tokens[3], /^__loopt_step_[0-9]+__$/);
  assert.equal(new Set(tokens).size, tokens.length);
});

test("tokenKind recognizes only complete tokens", () => {
  const jobToken = newToken("job");
  const stepToken = newToken("step");

  assert.equal(tokenKind(jobToken), "job");
  assert.equal(tokenKind(stepToken), "step");
  assert.equal(tokenKind("build"), undefined);
  assert.equal(tokenKind("__loopt_job___"), undefined);
  assert.equal(tokenKind("__loopt_job_1_"), undefined);
  assert.equal(tokenKind("prefix" + jobToken), undefined);
});

test("tokensIn lists unique tokens in order of appearance", () => {
  const first = newToken("job");
  const second = newToken("step");

  assert.deepEqual(tokensIn(`${first} then ${second} then ${first}`), [first, second]);
  assert.deepEqual(tokensIn("no tokens here"), []);
  assert.deepEqual(tokensIn(""), []);
});

test("collectTokens walks documents but not references or class instances", () => {
  const jobToken = newToken("job");
  const stepToken = newToken("step");
  const expression = new Expr(`needs.${jobToken}.outputs.version`);
  const value = {
    text: `first ${stepToken}`,
    nested: [
      expression,
      [`second ${jobToken}`, ordered([["key", `third ${stepToken}`]])],
    ],
  };

  assert.deepEqual([...collectTokens(value)].sort(), [jobToken, stepToken].sort());

  const meta = uses("actions/meta@v1");
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });
  assert.equal(collectTokens({ meta, build }).size, 0);
  assert.equal(collectTokens(new Map([["key", `x ${jobToken}`]])).size, 0);

  const cyclic: { [key: string]: unknown } = { text: `x ${jobToken}` };
  cyclic.self = cyclic;
  assert.deepEqual([...collectTokens(cyclic)], [jobToken]);
});

test("substituteTokens rewrites values while keys and literal forms survive", () => {
  const jobToken = newToken("job");
  const stepToken = newToken("step");
  const document: DocValue = ordered([
    ["2", `second ${stepToken}`],
    ["1", raw("123456789012345678901234567890")],
    ["expr", new Expr(`needs.${jobToken}.outputs.version`)],
    ["list", [`${jobToken} ${stepToken}`]],
    ["key_" + jobToken, jobToken],
  ]);

  const replaced = substituteTokens(document, (token) => {
    if (token === jobToken) {
      return "build";
    }
    if (token === stepToken) {
      return "step_1";
    }
    return undefined;
  });

  assert.ok(replaced instanceof OrderedObject);
  const entries = replaced.entries;
  assert.deepEqual(entries.map(([key]) => key), [
    "2",
    "1",
    "expr",
    "list",
    "key_" + jobToken,
  ]);
  assert.equal(entries[1][1] instanceof RawNumber, true);
  assert.equal((entries[1][1] as RawNumber).literal, "123456789012345678901234567890");
  assert.equal(entries[2][1] instanceof Expr, true);
  assert.equal((entries[2][1] as Expr).source, "needs.build.outputs.version");
  assert.notEqual(entries[3][1], (document as OrderedObject).entries[3][1]);
  assert.equal(
    stringify(replaced),
    '{"2":"second step_1","1":123456789012345678901234567890,"expr":"${{ needs.build.outputs.version }}","list":["build step_1"],"key_' +
      jobToken +
      '":"build"}',
  );
  assert.equal((document as OrderedObject).entries[0][1], `second ${stepToken}`);
});

test("substituteTokens rejects tokens without a replacement", () => {
  const jobToken = newToken("job");

  throwsCode("unresolved-reference", () =>
    substituteTokens(`value ${jobToken}`, () => undefined),
  );
  throwsCode("unresolved-reference", () =>
    substituteTokens(new Expr(`needs.${jobToken}.result`), () => undefined),
  );
});

test("the canonical reference example lowers byte for byte", () => {
  const meta = run(`echo "version=1.2.3" >> "$GITHUB_OUTPUT"`, { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta, run("cargo build --release")],
  });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: build.outputs.version } })],
  });

  assert.equal(
    documentJson(workflow("release", { on: "push", jobs: { build, deploy } })),
    '{"name":"release","on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","run":"echo \\"version=1.2.3\\" >> \\"$GITHUB_OUTPUT\\""},{"run":"cargo build --release"}]},"deploy":{"runs-on":"ubuntu-latest","needs":"build","steps":[{"run":"./deploy.sh","env":{"VERSION":"${{ needs.build.outputs.version }}"}}]}}}',
  );
});

test("an explicit need and a derived reference to the same job stay single", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });
  const byReference = job({
    runsOn: "ubuntu-latest",
    needs: build,
    steps: [run("./deploy.sh", { env: { VERSION: build.outputs.version } })],
  });
  const byId = job({
    runsOn: "ubuntu-latest",
    needs: "build",
    steps: [run("./deploy.sh", { env: { VERSION: build.outputs.version } })],
  });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { build, reference: byReference, id: byId },
      }),
    ),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","run":"make"}]},"reference":{"runs-on":"ubuntu-latest","needs":"build","steps":[{"run":"./deploy.sh","env":{"VERSION":"${{ needs.build.outputs.version }}"}}]},"id":{"runs-on":"ubuntu-latest","needs":"build","steps":[{"run":"./deploy.sh","env":{"VERSION":"${{ needs.build.outputs.version }}"}}]}}}',
  );
});

test("several derived needs keep first-seen order after explicit ones", () => {
  const buildMeta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: buildMeta.outputs.version },
    steps: [buildMeta],
  });
  const testMeta = run("test", { outputs: ["status"] });
  const testJob = job({
    runsOn: "ubuntu-latest",
    outputs: { status: testMeta.outputs.status },
    steps: [testMeta],
  });
  const deploy = job({
    runsOn: "ubuntu-latest",
    needs: build,
    steps: [
      run("./lint.sh", { env: { TEST: testJob.outputs.status } }),
      run("./ship.sh", { env: { VERSION: build.outputs.version } }),
    ],
  });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { build, test: testJob, deploy },
      }),
    ),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","run":"make"}]},"test":{"runs-on":"ubuntu-latest","outputs":{"status":"${{ steps.step_1.outputs.status }}"},"steps":[{"id":"step_1","run":"test"}]},"deploy":{"runs-on":"ubuntu-latest","needs":["build","test"],"steps":[{"run":"./lint.sh","env":{"TEST":"${{ needs.test.outputs.status }}"}},{"run":"./ship.sh","env":{"VERSION":"${{ needs.build.outputs.version }}"}}]}}}',
  );
});

test("a job result derives the need and renders bare in if", () => {
  const build = job({ runsOn: "ubuntu-latest", steps: [run("make")] });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { if: eq(build.result, "success") })],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build, deploy } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"run":"make"}]},"deploy":{"runs-on":"ubuntu-latest","needs":"build","steps":[{"if":"needs.build.result == \'success\'","run":"./deploy.sh"}]}}}',
  );
});

test("template interpolation substitutes real ids and derives the needs", () => {
  const buildMeta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: buildMeta.outputs.version },
    steps: [buildMeta],
  });
  const testMeta = run("test", { outputs: ["status"] });
  const testJob = job({
    runsOn: "ubuntu-latest",
    outputs: { status: testMeta.outputs.status },
    steps: [testMeta],
  });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [
      run(`deploy ${build.outputs.version}`, {
        env: { STATUS: `status=${testJob.outputs.status}` },
      }),
    ],
  });

  assert.equal(
    documentJson(
      workflow(undefined, {
        on: "push",
        jobs: { build, test: testJob, deploy },
      }),
    ),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","run":"make"}]},"test":{"runs-on":"ubuntu-latest","outputs":{"status":"${{ steps.step_1.outputs.status }}"},"steps":[{"id":"step_1","run":"test"}]},"deploy":{"runs-on":"ubuntu-latest","needs":["build","test"],"steps":[{"run":"deploy ${{ needs.build.outputs.version }}","env":{"STATUS":"status=${{ needs.test.outputs.status }}"}}]}}}',
  );
});

test("a reference in a callWorkflow with derives the need", () => {
  const meta = uses("actions/meta@v1");
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });
  const reuse = callWorkflow("./.github/workflows/reuse.yml", {
    needs: [],
    with: { version: build.outputs.version },
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build, reuse } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","uses":"actions/meta@v1"}]},"reuse":{"uses":"./.github/workflows/reuse.yml","needs":"build","with":{"version":"${{ needs.build.outputs.version }}"}}}}',
  );
});

test("callWorkflow outputs accept any name", () => {
  const reuse = callWorkflow("./.github/workflows/reuse.yml");
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: reuse.outputs.anything } })],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { reuse, deploy } })),
    '{"on":"push","jobs":{"reuse":{"uses":"./.github/workflows/reuse.yml"},"deploy":{"runs-on":"ubuntu-latest","needs":"reuse","steps":[{"run":"./deploy.sh","env":{"VERSION":"${{ needs.reuse.outputs.anything }}"}}]}}}',
  );
});

test("an undeclared job output is rejected where it is read", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });

  const error = captureError(
    () => (build.outputs as unknown as Record<string, Expr<string>>).verison,
  );
  assert.equal(error.code, "undeclared-output");
  assert.ok(error.message.includes("verison"), error.message);
  assert.ok(error.message.includes("version"), error.message);

  const plain = job({ runsOn: "ubuntu-latest", steps: [run("make")] });
  throwsCode("undeclared-output", () =>
    (plain.outputs as unknown as Record<string, Expr<string>>).anything,
  );
});

test("run step outputs must be declared", () => {
  const declared = run("make", { outputs: ["version"] });
  assert.equal(
    declared.outputs.version.source,
    `steps.${declared.token}.outputs.version`,
  );
  throwsCode("undeclared-output", () =>
    (declared.outputs as unknown as Record<string, Expr<string>>).other,
  );

  const undeclared = run("plain");
  throwsCode("undeclared-output", () =>
    (undeclared.outputs as unknown as Record<string, Expr<string>>).anything,
  );
});

test("uses step outputs are free-form unless declared", () => {
  const open = uses("actions/open@v1");
  assert.equal(
    open.outputs.anything.source,
    `steps.${open.token}.outputs.anything`,
  );

  const closed = uses("actions/closed@v1", undefined, { outputs: ["version"] });
  assert.equal(
    closed.outputs.version.source,
    `steps.${closed.token}.outputs.version`,
  );
  throwsCode("undeclared-output", () =>
    (closed.outputs as unknown as Record<string, Expr<string>>).other,
  );

  const build = job({
    runsOn: "ubuntu-latest",
    steps: [open, run("use", { env: { V: open.outputs.anything } })],
  });
  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"id":"step_1","uses":"actions/open@v1"},{"run":"use","env":{"V":"${{ steps.step_1.outputs.anything }}"}}]}}}',
  );
});

test("the outputs option never appears on a step", () => {
  const meta = run("make", { name: "Meta", outputs: ["version"] });
  const closed = uses("actions/closed@v1", undefined, { outputs: ["result"] });
  const build = job({
    runsOn: "ubuntu-latest",
    steps: [
      meta,
      closed,
      run("use", {
        env: {
          VERSION: meta.outputs.version,
          RESULT: closed.outputs.result,
        },
      }),
    ],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"name":"Meta","id":"step_1","run":"make"},{"id":"step_2","uses":"actions/closed@v1"},{"run":"use","env":{"VERSION":"${{ steps.step_1.outputs.version }}","RESULT":"${{ steps.step_2.outputs.result }}"}}]}}}',
  );
});

test("a job that reads its own output is rejected", () => {
  const meta = run("make", { outputs: ["version"] });
  const steps: Array<StepRef | StepRef[]> = [meta];
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps,
  });
  steps.push(run("./use.sh", { env: { VERSION: build.outputs.version } }));

  throwsCode("self-reference", () =>
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
  );
});

test("a JobRef placed in another workflow is unknown here", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });
  documentJson(workflow("other", { on: "push", jobs: { build } }));

  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: build.outputs.version } })],
  });
  throwsCode("unknown-job-reference", () =>
    documentJson(workflow("release", { on: "push", jobs: { deploy } })),
  );
});

test("an explicit string need must name a job in the workflow", () => {
  const build = job({
    runsOn: "ubuntu-latest",
    needs: "ghost",
    steps: [run("make")],
  });

  throwsCode("unknown-job-reference", () =>
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
  );
});

test("an explicit needs cycle is rejected with both ids", () => {
  const alpha = job({
    runsOn: "ubuntu-latest",
    needs: "beta",
    steps: [run("alpha")],
  });
  const beta = job({
    runsOn: "ubuntu-latest",
    needs: "alpha",
    steps: [run("beta")],
  });

  const error = captureError(() =>
    documentJson(workflow(undefined, { on: "push", jobs: { alpha, beta } })),
  );
  assert.equal(error.code, "needs-cycle");
  assert.ok(error.message.includes("alpha"), error.message);
  assert.ok(error.message.includes("beta"), error.message);
  assert.ok(error.message.includes("->"), error.message);
});

test("a cycle created through derived references is rejected", () => {
  const alphaSteps: Array<StepRef | StepRef[]> = [];
  const betaSteps: Array<StepRef | StepRef[]> = [];
  const alpha = job({ runsOn: "ubuntu-latest", steps: alphaSteps });
  const beta = job({ runsOn: "ubuntu-latest", steps: betaSteps });

  alphaSteps.push(run("./alpha.sh", { if: eq(beta.result, "success") }));
  betaSteps.push(run("./beta.sh", { if: eq(alpha.result, "success") }));

  const error = captureError(() =>
    documentJson(workflow(undefined, { on: "push", jobs: { alpha, beta } })),
  );
  assert.equal(error.code, "needs-cycle");
  assert.ok(error.message.includes("alpha"), error.message);
  assert.ok(error.message.includes("beta"), error.message);
  assert.ok(error.message.includes("->"), error.message);
});

test("a step reference cannot cross jobs", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({ runsOn: "ubuntu-latest", steps: [meta] });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: meta.outputs.version } })],
  });

  const error = captureError(() =>
    documentJson(workflow(undefined, { on: "push", jobs: { build, deploy } })),
  );
  assert.equal(error.code, "step-ref-cross-job");
  assert.ok(error.message.includes("outputs"), error.message);
});

test("a step reference outside the workflow is rejected", () => {
  const meta = run("make", { outputs: ["version"] });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: meta.outputs.version } })],
  });

  throwsCode("unknown-step-reference", () =>
    documentJson(workflow(undefined, { on: "push", jobs: { deploy } })),
  );
});

test("only referenced steps receive automatic ids", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta, run("cargo build --release")],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1.outputs.version }}"},"steps":[{"id":"step_1","run":"make"},{"run":"cargo build --release"}]}}}',
  );
});

test("a taken explicit id pushes the automatic id to the next suffix", () => {
  const first = run("make", { outputs: ["version"] });
  const second = run("test", { id: "step_1" });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: first.outputs.version },
    steps: [first, second],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","outputs":{"version":"${{ steps.step_1_1.outputs.version }}"},"steps":[{"id":"step_1_1","run":"make"},{"id":"step_1","run":"test"}]}}}',
  );
});

test("automatic ids stay unique per job and reach steps inside parallel", () => {
  const before = run("before", { outputs: ["value"] });
  const inner = uses("actions/inner@v1");
  const build = job({
    runsOn: "ubuntu-latest",
    steps: [
      before,
      parallel(inner, uses("actions/other@v1")),
      run("after", {
        env: { BEFORE: before.outputs.value, INNER: inner.outputs.value },
      }),
    ],
  });

  const first = uses("actions/first@v1");
  const second = uses("actions/second@v1");
  const testSteps: Array<StepRef | StepRef[]> = [
    first,
    second,
    uses("actions/third@v1"),
  ];
  testSteps.push(run("use", { env: { FIRST: first.outputs.x, SECOND: second.outputs.y } }));
  const testJob = job({ runsOn: "ubuntu-latest", steps: testSteps });

  assert.equal(
    documentJson(
      workflow(undefined, { on: "push", jobs: { build, test: testJob } }),
    ),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"id":"step_1","run":"before"},{"parallel":[{"id":"step_3","uses":"actions/inner@v1"},{"uses":"actions/other@v1"}]},{"run":"after","env":{"BEFORE":"${{ steps.step_1.outputs.value }}","INNER":"${{ steps.step_3.outputs.value }}"}}]},"test":{"runs-on":"ubuntu-latest","steps":[{"id":"step_1","uses":"actions/first@v1"},{"id":"step_2","uses":"actions/second@v1"},{"uses":"actions/third@v1"},{"run":"use","env":{"FIRST":"${{ steps.step_1.outputs.x }}","SECOND":"${{ steps.step_2.outputs.y }}"}}]}}}',
  );
});

test("outcome and conclusion render in if expressions", () => {
  const buildStep = run("make");
  const build = job({
    runsOn: "ubuntu-latest",
    steps: [
      buildStep,
      run("./on-success.sh", { if: eq(buildStep.outcome, "success") }),
      run("./on-failure.sh", { if: eq(buildStep.conclusion, "failure") }),
    ],
  });

  assert.equal(
    documentJson(workflow(undefined, { on: "push", jobs: { build } })),
    '{"on":"push","jobs":{"build":{"runs-on":"ubuntu-latest","steps":[{"id":"step_1","run":"make"},{"if":"steps.step_1.outcome == \'success\'","run":"./on-success.sh"},{"if":"steps.step_1.conclusion == \'failure\'","run":"./on-failure.sh"}]}}}',
  );
});

test("assembling twice is stable and leaves no tokens behind", () => {
  const meta = run("make", { outputs: ["version"] });
  const build = job({
    runsOn: "ubuntu-latest",
    outputs: { version: meta.outputs.version },
    steps: [meta],
  });
  const deploy = job({
    runsOn: "ubuntu-latest",
    steps: [run("./deploy.sh", { env: { VERSION: build.outputs.version } })],
  });
  const target = workflow("repeat", { on: "push", jobs: { build, deploy } });

  const first = documentJson(target);
  const second = documentJson(target);

  assert.equal(first, second);
  assert.equal(first.includes("__loopt_"), false);
});
