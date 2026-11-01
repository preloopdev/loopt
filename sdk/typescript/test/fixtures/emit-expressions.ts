// Emits a JSON workflow whose step environment values are built expressions,
// for the expression-syntax cross-check in `tests/sdk_expressions.rs`.
//
// Node executes this file directly through its built-in type stripping, so
// only erasable TypeScript syntax is allowed here. Run it from the repository
// root with:
//
//     node sdk/typescript/test/fixtures/emit-expressions.ts valid
//     node sdk/typescript/test/fixtures/emit-expressions.ts invalid
//
// `valid` covers every operator, every function, every exported context, and
// several camelCase context aliases. `invalid` carries one deliberately
// malformed raw expression so the linter has something to reject.

import { emit, rawExpr } from "../../src/kit/index.ts";
import type { DocValue, Expr } from "../../src/kit/index.ts";
import {
  always,
  and,
  at,
  cancelled,
  caseOf,
  contains,
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
  or,
  runner,
  secrets,
  star,
  startsWith,
  strategy,
  success,
  toJSON,
  vars,
} from "../../src/expr/index.ts";

/** Minimal view of Node's `process`; the fixture only reads its arguments. */
declare const process: { argv: string[] };

/** Every expression emitted by `valid`, in document order. */
const validExpressions: Expr[] = [
  // Literals, including a string with a doubled quote.
  lit("plain"),
  lit("it's"),
  lit(42),
  lit(1.5),
  lit(true),
  lit(false),
  lit(null),
  // Every exported context and a few camelCase aliases.
  github.ref,
  github.event.pull_request.title,
  github.runId,
  github.refName,
  github.repositoryOwner,
  github.eventName,
  github.sha,
  env.HOME,
  env.at("MY VAR"),
  vars.MY_VAR,
  secrets.GITHUB_TOKEN,
  inputs.name,
  inputs.at("with-dash"),
  runner.os,
  runner.toolCache,
  strategy.failFast,
  strategy.jobIndex,
  strategy.maxParallel,
  jobContext.status,
  // Comparison operators.
  eq(github.ref, lit("refs/heads/main")),
  ne(github.ref, lit("refs/heads/main")),
  lt(github.runNumber, lit(10)),
  lte(github.runNumber, lit(10)),
  gt(github.runNumber, lit(1)),
  gte(github.runNumber, lit(1)),
  // Boolean operators, including nesting that needs parentheses.
  and(github.ref, github.sha),
  or(github.ref, github.sha),
  not(github.event),
  and(or(eq(github.ref, lit("a")), eq(github.ref, lit("b"))), success()),
  // Member access.
  at(github, "event"),
  star(github.event),
  // Every function.
  contains(github.ref, lit("main")),
  startsWith(github.ref, lit("refs/heads/")),
  endsWith(github.ref, lit("/main")),
  format(lit("{0}/{1}"), github.repository, github.sha),
  join(fromJSON(lit('["a","b"]')), lit(", ")),
  toJSON(github),
  fromJSON(lit('{"a":1}')),
  hashFiles("**/*.ts", "sdk/**/*.ts"),
  caseOf(eq(github.event_name, lit("push")), lit("p"), lit("other")),
  success(),
  failure(),
  always(),
  cancelled(),
];

/** Builds the workflow document whose single step carries `expressions` as env values. */
function workflow(expressions: Expr[]): DocValue {
  const values: { [key: string]: DocValue } = {};
  expressions.forEach((expression, index) => {
    values["E" + String(index)] = expression;
  });
  return {
    on: "push",
    jobs: {
      t: {
        "runs-on": "ubuntu-latest",
        steps: [{ run: "true", env: values }],
      },
    },
  };
}

const mode = process.argv[2];
if (mode === "valid") {
  emit(workflow(validExpressions));
} else if (mode === "invalid") {
  emit(workflow([rawExpr("github.ref ==")]));
} else {
  throw new Error("usage: emit-expressions.ts valid|invalid");
}
