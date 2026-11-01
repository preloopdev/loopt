// Behavioural tests for the hand-written TypeScript authoring kit.
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
  OrderedObject,
  Precedence,
  applyExtra,
  emit,
  isExpr,
  isIntegerLikeKey,
  lines,
  obj,
  ordered,
  raw,
  rawExpr,
  stringify,
} from "../src/kit/index.ts";

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

// `stringify` only accepts `DocValue`s; these negative tests probe runtime
// values outside that domain (undefined, functions, bigints, symbols).
const stringifyAnything = stringify as (value: unknown) => string;

test("LooptError carries a code and reports its class name", () => {
  const error = new LooptError("sample-code", "sample message");
  assert.ok(error instanceof Error);
  assert.equal(error.name, "LooptError");
  assert.equal(error.code, "sample-code");
  assert.equal(error.message, "sample message");
});

test("stringify preserves plain-object insertion order", () => {
  assert.equal(
    stringify({ b: 1, a: 2, c: 3 }),
    '{"b":1,"a":2,"c":3}',
  );
});

test("ordered keeps entry order where a plain object would reorder integer-like keys", () => {
  assert.equal(
    stringify(ordered([["2", 1], ["a", 2], ["1", 3]])),
    '{"2":1,"a":2,"1":3}',
  );
  // A plain object literal hoists integer-like keys ahead of the rest, in
  // ascending numeric order, no matter how they were written.
  assert.equal(
    stringify({ "2": 1, a: 2, "1": 3 }),
    '{"1":3,"2":1,"a":2}',
  );
});

test("obj uses OrderedObject for integer-like and __proto__ keys", () => {
  const numeric = obj([["2", 1], ["a", 2], ["1", 3]]);
  assert.ok(numeric instanceof OrderedObject);
  assert.equal(stringify(numeric), '{"2":1,"a":2,"1":3}');

  const proto = obj([["__proto__", { polluted: true }], ["a", 1]]);
  assert.ok(proto instanceof OrderedObject);
  assert.equal(Object.getPrototypeOf(proto), OrderedObject.prototype);
  assert.equal(
    stringify(proto),
    '{"__proto__":{"polluted":true},"a":1}',
  );
  const probe: { polluted?: unknown } = {};
  assert.equal(probe.polluted, undefined);
});

test("obj returns a plain object when no key needs the ordered representation", () => {
  const plain = obj([["a", 1], ["b", 2]]);
  assert.ok(!(plain instanceof OrderedObject));
  assert.equal(stringify(plain), '{"a":1,"b":2}');
});

test("ordered and obj reject duplicate keys", () => {
  throwsCode("duplicate-key", () => ordered([["a", 1], ["a", 2]]));
  throwsCode("duplicate-key", () => obj([["a", 1], ["a", 2]]));
});

test("raw preserves exact numeric text and enforces JSON number grammar", () => {
  assert.equal(
    stringify(raw("12345678901234567890")),
    "12345678901234567890",
  );
  assert.equal(stringify(raw("1.5e3")), "1.5e3");
  assert.equal(stringify(raw("-0.25E+2")), "-0.25E+2");

  for (const literal of ["01", "1.", "abc"]) {
    throwsCode("invalid-number", () => raw(literal));
  }
});

test("stringify rejects non-finite numbers and unsupported runtime values", () => {
  for (const value of [
    Number.NaN,
    Number.POSITIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
  ]) {
    throwsCode("non-finite-number", () => stringifyAnything(value));
  }

  for (const value of [undefined, () => 1, 1n, Symbol("kit")]) {
    throwsCode("unsupported-value", () => stringifyAnything(value));
  }
});

test("stringify escapes quotes, backslashes, and control characters", () => {
  assert.equal(stringify('he said "hi"'), '"he said \\"hi\\""');
  assert.equal(stringify("back\\slash"), '"back\\\\slash"');
  assert.equal(stringify("\n\r\t\b\f"), '"\\n\\r\\t\\b\\f"');
  assert.equal(stringify("\u0000\u0001"), '"\\u0000\\u0001"');
  assert.equal(stringify("\u001f").toLowerCase(), '"\\u001f"');
});

test("stringify leaves non-ASCII text readable and escapes lone surrogates", () => {
  assert.equal(stringify("héllo → 😀 日本語"), '"héllo → 😀 日本語"');
  assert.equal(stringify("\uD800").toLowerCase(), '"\\ud800"');
  assert.equal(stringify("a\uDC00b").toLowerCase(), '"a\\udc00b"');
});

test("isIntegerLikeKey accepts only canonical array-index strings", () => {
  for (const key of ["0", "10", "4294967294"]) {
    assert.equal(isIntegerLikeKey(key), true, key);
  }
  for (const key of ["01", "-1", "1.5", "", "4294967295"]) {
    assert.equal(isIntegerLikeKey(key), false, key);
  }
});

test("lines joins items without a trailing newline", () => {
  assert.equal(lines("a", "b"), "a\nb");
  assert.equal(lines("solo"), "solo");
  assert.equal(lines(), "");
});

test("Expr renders as an expression call and exposes its bare source", () => {
  const expression = new Expr("github.ref == 'refs/heads/main'");
  assert.equal(expression.bare(), "github.ref == 'refs/heads/main'");
  assert.equal(
    expression.toString(),
    "${{ github.ref == 'refs/heads/main' }}",
  );
  assert.equal(expression.precedence, Precedence.Atom);

  const defaults = new Expr("1 == 1");
  assert.equal(defaults.contexts.size, 0);
  assert.equal(defaults.precedence, Precedence.Atom);

  const configured = new Expr("github.sha", {
    contexts: ["github"],
    precedence: Precedence.Member,
  });
  assert.equal(configured.precedence, Precedence.Member);
  assert.equal(configured.contexts.size, 1);
  assert.ok(configured.contexts.has("github"));
  assert.equal(configured.toString(), "${{ github.sha }}");
});

test("Expr rejects empty sources and delimiters outside string literals", () => {
  for (const source of ["", "   ", "a }} b", "${{ x }}", "a ${{ b"]) {
    throwsCode("invalid-expression", () => new Expr(source));
  }
});

test("Expr accepts delimiters inside single-quoted string literals", () => {
  assert.doesNotThrow(() => new Expr("format('}}', github.sha)"));
  assert.doesNotThrow(() => new Expr("format('${{', github.sha)"));
  assert.doesNotThrow(() => new Expr("format('it''s }} fine', github.sha)"));
  assert.equal(
    new Expr("format('}}', github.sha)").bare(),
    "format('}}', github.sha)",
  );
});

test("an Expr inside a document stringifies as its expression text", () => {
  assert.equal(
    stringify({ expr: new Expr("github.sha") }),
    '{"expr":"${{ github.sha }}"}',
  );
});

test("rawExpr and isExpr identify expressions", () => {
  const expression = rawExpr("github.sha");
  assert.ok(expression instanceof Expr);
  assert.equal(expression.bare(), "github.sha");
  assert.equal(isExpr(expression), true);
  assert.equal(isExpr(new Expr("x")), true);

  for (const value of [null, undefined, "github.sha", 1, {}, []]) {
    assert.equal(isExpr(value), false);
  }
});

test("applyExtra appends extras in their own key order", () => {
  const merged = applyExtra([["name", "ci"]], {
    permissions: "read-all",
    concurrency: "ci",
  });
  assert.deepEqual(merged, [
    ["name", "ci"],
    ["permissions", "read-all"],
    ["concurrency", "ci"],
  ]);
});

test("applyExtra leaves entries untouched for missing or empty extras", () => {
  const entries: Array<[string, string]> = [["name", "ci"]];
  assert.equal(applyExtra(entries, undefined), entries);
  assert.equal(applyExtra(entries, {}), entries);
});

test("applyExtra rejects extras that repeat a known key", () => {
  throwsCode("extra-conflict", () =>
    applyExtra([["name", "ci"]], { name: "other" }),
  );
});

test("emit writes exactly one newline-terminated document", () => {
  const chunks: string[] = [];
  const write = (text: string): void => {
    chunks.push(text);
  };

  emit({ name: "ci" }, write);
  assert.deepEqual(chunks, ['{"name":"ci"}\n']);
});

test("emit resolves toDocument() before serialising", () => {
  const chunks: string[] = [];
  const write = (text: string): void => {
    chunks.push(text);
  };

  emit({ toDocument: () => ({ name: "ci" }) }, write);
  assert.deepEqual(chunks, ['{"name":"ci"}\n']);
});

test("emit rejects documents that are not objects", () => {
  for (const value of [[], "text", null, 1, true]) {
    const chunks: string[] = [];
    const write = (text: string): void => {
      chunks.push(text);
    };

    throwsCode("emit-not-object", () => emit(value, write));
    assert.deepEqual(chunks, []);
  }
});
