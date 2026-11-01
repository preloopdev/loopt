// Tests for safe tagged run templates. The generated command is executed with
// shell values to prove that interpolation data remains data, not shell source.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";

import { LooptError, github, rawExpr, run } from "../src/index.ts";
import type { StepRef } from "../src/index.ts";

type StepDefinition = { run: string; env?: Record<string, unknown> };

function definition(step: StepRef): StepDefinition {
  return step.def as unknown as StepDefinition;
}

function generatedEnvironment(step: StepRef): [string, unknown][] {
  const env = definition(step).env;
  const command = definition(step).run;
  assert.ok(env !== undefined, "safe interpolation should create a step environment");
  return Object.entries(env).filter(
    ([name]) => name.startsWith("__LOOPT_RUN_") && command.includes(name),
  );
}

function execute(step: StepRef): string {
  const def = definition(step);
  const generated = generatedEnvironment(step);
  const environment: Record<string, string> = {};
  for (const [name, value] of generated) {
    environment[name] = String(value);
  }
  return execFileSync("/bin/sh", ["-c", def.run], {
    env: environment,
    encoding: "utf8",
  });
}

function throwsLoopt(action: () => unknown, code?: string): void {
  assert.throws(action, (error: unknown) => {
    assert.ok(error instanceof LooptError);
    if (code !== undefined) {
      assert.equal(error.code, code);
    }
    return true;
  });
}

test("safe interpolation cannot execute a malicious shell payload", () => {
  const payload = "$(printf injected >&2); touch /tmp/loopt-safe-run-marker";
  const step = run`printf '<%s>\\n' ${payload}`;

  assert.equal(execute(step), `<${payload}>\n`);
});

test("spaces, quotes, metacharacters, repeated values, and literal fragments stay intact", () => {
  const first = "two words 'quoted'";
  const second = "$(not-a-command); && *";
  const step = run`printf '%s' prefix-${first}-${first}-${second}-suffix`;

  assert.equal(
    execute(step),
    `prefix-${first}-${first}-${second}-suffix`,
  );
});

test("double-quoted and unquoted literal contexts preserve one shell argument", () => {
  const value = "a b; c";
  const quoted = run`printf '<%s>\\n' "left ${value} right"`;
  const unquoted = run`printf '<%s>\\n' left-${value}-right`;

  assert.equal(execute(quoted), `<left ${value} right>\n`);
  assert.equal(execute(unquoted), `<left-${value}-right>\n`);
});

test("generated environment names avoid user environment collisions", () => {
  const step = run({ env: { __LOOPT_RUN_0: "existing" } })`printf '%s' ${"value"}`;
  const def = definition(step);
  const generated = generatedEnvironment(step);

  assert.deepEqual(generated.map(([name]) => name), ["__LOOPT_RUN_1"]);
  assert.equal(def.env?.__LOOPT_RUN_0, "existing");
  assert.equal(execute(step), "value");
});

test("expression objects remain in env values for normal lowering", () => {
  const title = github.event.pullRequest.title;
  const step = run`printf '%s' ${title}`;
  const generated = generatedEnvironment(step);

  assert.equal(generated.length, 1);
  assert.equal(generated[0]?.[1], title);
  assert.equal(String(generated[0]?.[1]), "${{ github.event.pullRequest.title }}");
});

test("safe tagged runs preserve options and typed outputs", () => {
  const step = run({ id: "build", outputs: ["version"], shell: "bash" })`printf '%s' ${"ok"}`;

  assert.equal(step.id, "build");
  assert.match(step.outputs.version.source, /^steps\.__loopt_step_[0-9]+__\.outputs\.version$/);
  assert.equal(definition(step).run, `printf '%s' "\${__LOOPT_RUN_0}"`);
});

test("ordinary run and explicit unsafe run keep direct source behavior", () => {
  const ordinary = run("echo literal", { id: "ordinary" });
  const unsafe = run.unsafe`echo ${"$(intentional)"}`;

  assert.equal(definition(ordinary).run, "echo literal");
  assert.equal(ordinary.id, "ordinary");
  assert.equal(definition(unsafe).run, "echo $(intentional)");
});

test("safe templates reject ambiguous quoting and unsupported shells", () => {
  throwsLoopt(() => run`printf '%s' '${"value"}'`, "ambiguous-shell-context");
  throwsLoopt(() => run`printf '%s' $(echo ${"value"})`, "ambiguous-shell-context");
  throwsLoopt(
    () => run({ shell: "pwsh" })`Write-Output ${"value"}`,
    "unsupported-shell",
  );
});

test("safe templates reject non-value interpolations", () => {
  throwsLoopt(() => run`echo ${undefined}`, "invalid-run-interpolation");
  throwsLoopt(() => run`echo ${null as unknown as string}`, "invalid-run-interpolation");
});
