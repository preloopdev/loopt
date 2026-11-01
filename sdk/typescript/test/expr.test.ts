// Behavioural tests for the hand-written GitHub Actions expression builders.
//
// Node executes this file directly through its built-in type stripping, so
// only erasable TypeScript syntax is allowed here. Run the suite from the
// repository root with:
//
//     node --test "sdk/typescript/test/*.test.ts"

import assert from "node:assert/strict";
import test from "node:test";

import {
  Expr,
  LooptError,
  Precedence,
  isExpr,
  rawExpr,
  stringify,
} from "../src/kit/index.ts";
import {
  always,
  and,
  at,
  cancelled,
  caseOf,
  contains,
  context,
  endsWith,
  env,
  eq,
  failure,
  format,
  fromJSON,
  github,
  gt,
  gte,
  hashFiles,
  inputs,
  jobContext,
  join,
  lit,
  lt,
  lte,
  ne,
  not,
  operand,
  or,
  runner,
  secrets,
  star,
  startsWith,
  strategy,
  success,
  toJSON,
  vars,
} from "../src/expr/index.ts";
import type { DynExpr } from "../src/expr/index.ts";

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

/** A bare atom expression used as an operator operand. */
const a = rawExpr("a");
/** A second bare atom expression. */
const b = rawExpr("b");
/** A third bare atom expression. */
const c = rawExpr("c");
/** A fourth bare atom expression. */
const x = rawExpr("x");
/** A fifth bare atom expression, used for even-arity `caseOf` rejection. */
const d = rawExpr("d");

test("lit renders string, number, boolean, and null literals", () => {
  assert.equal(lit("it's").source, "'it''s'");
  assert.equal(lit("plain").source, "'plain'");
  assert.equal(lit("").source, "''");
  assert.equal(lit(42).source, "42");
  assert.equal(lit(1.5).source, "1.5");
  assert.equal(lit(-1.5).source, "-1.5");
  assert.equal(lit(true).source, "true");
  assert.equal(lit(false).source, "false");
  assert.equal(lit(null).source, "null");
});

test("lit uses atom precedence", () => {
  assert.equal(lit("v").precedence, Precedence.Atom);
  assert.equal(lit(0).precedence, Precedence.Atom);
  assert.equal(lit(true).precedence, Precedence.Atom);
  assert.equal(lit(null).precedence, Precedence.Atom);
});

test("lit rejects non-finite numbers", () => {
  throwsCode("non-finite-number", () => lit(Number.NaN));
  throwsCode("non-finite-number", () => lit(Number.POSITIVE_INFINITY));
  throwsCode("non-finite-number", () => lit(Number.NEGATIVE_INFINITY));
});

test("operand passes expressions through and lifts supported literals", () => {
  assert.equal(operand(a), a);
  const ref = github.ref;
  assert.equal(operand(ref), ref);
  assert.equal(operand("x").source, "'x'");
  assert.equal(operand(7).source, "7");
  assert.equal(operand(false).source, "false");
  assert.equal(operand(null).source, "null");
});

test("operand rejects values that have no expression form", () => {
  throwsCode("unsupported-value", () => operand(undefined));
  throwsCode("unsupported-value", () => operand({}));
  throwsCode("unsupported-value", () => operand([]));
  throwsCode("unsupported-value", () => operand(() => 1));
});

test("comparison operators render with GitHub's exact spelling", () => {
  assert.equal(eq(a, b).source, "a == b");
  assert.equal(ne(a, b).source, "a != b");
  assert.equal(lt(a, b).source, "a < b");
  assert.equal(lte(a, b).source, "a <= b");
  assert.equal(gt(a, b).source, "a > b");
  assert.equal(gte(a, b).source, "a >= b");
});

test("comparison operators carry their precedence level", () => {
  assert.equal(eq(a, b).precedence, Precedence.Equality);
  assert.equal(ne(a, b).precedence, Precedence.Equality);
  assert.equal(lt(a, b).precedence, Precedence.Comparison);
  assert.equal(lte(a, b).precedence, Precedence.Comparison);
  assert.equal(gt(a, b).precedence, Precedence.Comparison);
  assert.equal(gte(a, b).precedence, Precedence.Comparison);
});

test("comparison operands are parenthesised when they are not tighter", () => {
  assert.equal(eq(eq(a, b), c).source, "(a == b) == c");
  assert.equal(eq(a, eq(b, c)).source, "a == (b == c)");
  assert.equal(eq(lt(a, b), c).source, "a < b == c");
  assert.equal(lte(lt(a, b), c).source, "(a < b) <= c");
  assert.equal(lt(or(a, b), c).source, "(a || b) < c");
  assert.equal(eq(and(a, b), c).source, "(a && b) == c");
  assert.equal(eq(at(a, "k"), b).source, "a['k'] == b");
});

test("and and or join operands and flatten same-operator chains", () => {
  assert.equal(and(a, b).source, "a && b");
  assert.equal(or(a, b).source, "a || b");
  assert.equal(and(a, b, c).source, "a && b && c");
  assert.equal(or(a, b, c).source, "a || b || c");
  assert.equal(and(and(a, b), c).source, "a && b && c");
  assert.equal(or(or(a, b), c).source, "a || b || c");
  assert.equal(and(a, b, or(c, x)).source, "a && b && (c || x)");
});

test("and and or parenthesise looser operands only", () => {
  assert.equal(and(or(a, b), c).source, "(a || b) && c");
  assert.equal(and(a, or(b, c)).source, "a && (b || c)");
  assert.equal(or(and(a, b), c).source, "a && b || c");
  assert.equal(or(a, and(b, c)).source, "a || b && c");
  assert.equal(and(eq(a, b), c).source, "a == b && c");
});

test("and and or carry their precedence level", () => {
  assert.equal(and(a, b).precedence, Precedence.And);
  assert.equal(or(a, b).precedence, Precedence.Or);
});

test("and and or require at least two operands", () => {
  throwsCode("invalid-arity", () => and());
  throwsCode("invalid-arity", () => and(a));
  throwsCode("invalid-arity", () => or());
  throwsCode("invalid-arity", () => or(a));
});

test("not renders and parenthesises looser operands", () => {
  assert.equal(not(a).source, "!a");
  assert.equal(not(eq(a, b)).source, "!(a == b)");
  assert.equal(not(and(a, b)).source, "!(a && b)");
  assert.equal(not(not(x)).source, "!!x");
  assert.equal(not(at(a, "k")).source, "!a['k']");
  assert.equal(not(a).precedence, Precedence.Not);
});

test("at renders computed member access", () => {
  assert.equal(at(a, "k").source, "a['k']");
  assert.equal(at(a, 0).source, "a[0]");
  assert.equal(at(a, "it's").source, "a['it''s']");
  assert.equal(at(eq(a, b), "k").source, "(a == b)['k']");
  assert.equal(at(or(a, b), "k").source, "(a || b)['k']");
  assert.equal(at(a, "k").precedence, Precedence.Member);
});

test("at rejects keys with no expression form", () => {
  throwsCode("unsupported-value", () => at(a, {}));
});

test("star renders wildcard member access", () => {
  assert.equal(star(a).source, "a.*");
  assert.equal(star(at(a, "k")).source, "a['k'].*");
  assert.equal(star(github.event).source, "github.event.*");
  assert.equal(star(a).precedence, Precedence.Member);
});

test("functions render GitHub's spelling and join arguments with ', '", () => {
  assert.equal(contains(lit("abc"), lit("b")).source, "contains('abc', 'b')");
  assert.equal(
    startsWith(lit("abc"), lit("ab")).source,
    "startsWith('abc', 'ab')",
  );
  assert.equal(endsWith(lit("abc"), lit("bc")).source, "endsWith('abc', 'bc')");
  assert.equal(
    format(lit("{0}-{1}"), a, b).source,
    "format('{0}-{1}', a, b)",
  );
  assert.equal(join(a).source, "join(a)");
  assert.equal(join(a, lit(", ")).source, "join(a, ', ')");
  assert.equal(toJSON(a).source, "toJSON(a)");
  assert.equal(fromJSON(lit("{}")).source, "fromJSON('{}')");
  assert.equal(hashFiles("**/*.ts").source, "hashFiles('**/*.ts')");
  assert.equal(hashFiles("a", "b").source, "hashFiles('a', 'b')");
  assert.equal(caseOf(a, b, c).source, "case(a, b, c)");
  assert.equal(
    contains(at(a, "k"), lit("x")).source,
    "contains(a['k'], 'x')",
  );
});

test("functions use atom precedence", () => {
  assert.equal(contains(a, b).precedence, Precedence.Atom);
  assert.equal(format(lit("{0}"), a).precedence, Precedence.Atom);
  assert.equal(fromJSON(lit("{}")).precedence, Precedence.Atom);
});

test("functions reject wrong arities", () => {
  throwsCode("invalid-arity", () => contains(a));
  throwsCode("invalid-arity", () => contains(a, b, c));
  throwsCode("invalid-arity", () => startsWith(a));
  throwsCode("invalid-arity", () => startsWith(a, b, c));
  throwsCode("invalid-arity", () => endsWith(a));
  throwsCode("invalid-arity", () => endsWith(a, b, c));
  throwsCode("invalid-arity", () => format());
  throwsCode("invalid-arity", () => join());
  throwsCode("invalid-arity", () => join(a, b, c));
  throwsCode("invalid-arity", () => toJSON());
  throwsCode("invalid-arity", () => toJSON(a, b));
  throwsCode("invalid-arity", () => fromJSON());
  throwsCode("invalid-arity", () => fromJSON(a, b));
  throwsCode("invalid-arity", () => hashFiles());
  throwsCode("invalid-arity", () => caseOf(a, b));
  throwsCode("invalid-arity", () => caseOf(a, b, c, d));
});

test("function arity boundaries are accepted", () => {
  assert.equal(format(lit("{0}")).source, "format('{0}')");
  assert.equal(join(a, lit(",")).source, "join(a, ',')");
  assert.equal(caseOf(a, b, c).source, "case(a, b, c)");
  const patterns = Array.from({ length: 255 }, (_, index) => "p" + String(index));
  assert.equal(hashFiles(...patterns).source.split(", ").length, 255);
});

test("hashFiles rejects more than 255 patterns", () => {
  const patterns = Array.from({ length: 256 }, (_, index) => "p" + String(index));
  throwsCode("invalid-arity", () => hashFiles(...patterns));
});

test("status functions render empty argument lists and record their context", () => {
  assert.equal(success().source, "success()");
  assert.equal(failure().source, "failure()");
  assert.equal(always().source, "always()");
  assert.equal(cancelled().source, "cancelled()");

  const status: Array<[Expr, string]> = [
    [success(), "success"],
    [failure(), "failure"],
    [always(), "always"],
    [cancelled(), "cancelled"],
  ];
  for (const [expression, name] of status) {
    assert.deepEqual([...expression.contexts], [name]);
  }
});

test("status functions reject arguments", () => {
  throwsCode("invalid-arity", () => success(lit("x")));
  throwsCode("invalid-arity", () => failure(lit("x")));
  throwsCode("invalid-arity", () => always(lit("x")));
  throwsCode("invalid-arity", () => cancelled(lit("x")));
});

test("exported context roots render their names and lowercase context sets", () => {
  const roots: Array<[DynExpr, string]> = [
    [github, "github"],
    [env, "env"],
    [vars, "vars"],
    [secrets, "secrets"],
    [inputs, "inputs"],
    [runner, "runner"],
    [strategy, "strategy"],
    [jobContext, "job"],
  ];
  for (const [root, name] of roots) {
    assert.equal(root.source, name);
    assert.deepEqual([...root.contexts], [name]);
  }
});

test("context() builds a root from an arbitrary name", () => {
  const root = context("Steps");
  assert.equal(root.source, "Steps");
  assert.deepEqual([...root.contexts], ["steps"]);
});

test("dynamic member access renders dot form for identifier-like names", () => {
  assert.equal(github.ref.source, "github.ref");
  assert.equal(
    github.event.pull_request.title.source,
    "github.event.pull_request.title",
  );
  assert.equal(github.event.source, "github.event");
  assert.equal(
    context("steps")["build-app"].outputs.v.source,
    "steps.build-app.outputs.v",
  );
  assert.equal(github.ref.precedence, Precedence.Member);
});

test("dynamic member access quotes names that are not identifier-like", () => {
  const matrix = context("matrix");
  assert.equal(matrix["a.b"].source, "matrix['a.b']");
  assert.equal(matrix.at("a b").source, "matrix['a b']");
  assert.equal(matrix["it's"].source, "matrix['it''s']");
  assert.equal(matrix.at(0).source, "matrix[0]");
});

test("at() escapes payload names that collide with Expr members", () => {
  const escaped = github.at("source");
  assert.ok(isExpr(escaped));
  assert.equal(escaped.source, "github['source']");
  assert.equal(github.at("event").source, "github['event']");
});

test("camelCase aliases map to GitHub's snake_case and hyphenated members", () => {
  assert.equal(github.runId.source, "github.run_id");
  assert.equal(github.runNumber.source, "github.run_number");
  assert.equal(github.runAttempt.source, "github.run_attempt");
  assert.equal(github.refName.source, "github.ref_name");
  assert.equal(github.repositoryOwner.source, "github.repository_owner");
  assert.equal(github.eventName.source, "github.event_name");
  assert.equal(github.actorId.source, "github.actor_id");
  assert.equal(github.baseRef.source, "github.base_ref");
  assert.equal(github.headRef.source, "github.head_ref");
  assert.equal(github.workflowRef.source, "github.workflow_ref");
  assert.equal(strategy.failFast.source, "strategy.fail-fast");
  assert.equal(strategy.jobIndex.source, "strategy.job-index");
  assert.equal(strategy.maxParallel.source, "strategy.max-parallel");
  assert.equal(strategy.jobTotal.source, "strategy.job-total");
  assert.equal(runner.toolCache.source, "runner.tool_cache");
});

test("exact snake_case member names still work through the dynamic path", () => {
  assert.equal(github.run_id.source, "github.run_id");
  assert.equal(strategy["fail-fast"].source, "strategy.fail-fast");
  assert.equal(runner.tool_cache.source, "runner.tool_cache");
});

test("dynamic contexts are real Expr instances", () => {
  assert.ok(isExpr(github));
  assert.ok(github instanceof Expr);
  assert.ok(isExpr(github.ref));
  assert.ok(github.ref instanceof Expr);
  assert.ok(isExpr(github.event.pull_request));
  assert.ok(isExpr(github.at("event")));
  assert.equal(github.constructor, Expr);
});

test("dynamic contexts are not thenables", async () => {
  assert.equal(github.then, undefined);
  assert.equal(typeof github.valueOf, "function");
  const resolved = await Promise.resolve(github);
  assert.equal(resolved, github);
});

test("expressions expose the lowercase contexts they reference", () => {
  const condition = eq(github.ref, secrets.GITHUB_TOKEN);
  assert.deepEqual([...condition.contexts].sort(), ["github", "secrets"]);
  assert.deepEqual([...toJSON(github).contexts], ["github"]);
  assert.deepEqual([...hashFiles("**/*.ts").contexts], ["hashfiles"]);
  assert.deepEqual([...github.ref.contexts], ["github"]);
});

test("expressions render as ${{ ... }} strings inside documents", () => {
  assert.equal(stringify(github.ref), '"${{ github.ref }}"');
  assert.equal(
    stringify({ env: { E: github.ref } }),
    '{"env":{"E":"${{ github.ref }}"}}',
  );
  assert.equal(toJSON(github).toString(), "${{ toJSON(github) }}");
  assert.equal(github.ref.bare(), "github.ref");
  assert.equal(String(github.ref), "${{ github.ref }}");
});
