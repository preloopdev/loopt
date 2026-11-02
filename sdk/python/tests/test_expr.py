"""Unit tests for :mod:`loopt_sdk.expr`."""

from __future__ import annotations

import math
import unittest

from loopt_sdk.errors import LooptSDKError
from loopt_sdk.expr import (
    Expr,
    always,
    and_,
    at,
    cancelled,
    case_of,
    contains,
    ends_with,
    eq,
    failure,
    format,
    ge,
    github,
    gt,
    hash_files,
    inputs,
    join,
    le,
    lit,
    lt,
    matrix,
    ne,
    needs,
    not_,
    operand,
    or_,
    runner,
    secrets,
    star,
    starts_with,
    steps,
    strategy,
    success,
    to_json,
    from_json,
    vars,
    env,
    job_context,
)


class LiteralTests(unittest.TestCase):
    def test_strings_are_single_quoted_with_quotes_doubled(self) -> None:
        self.assertEqual(str(lit("main")), "${{ 'main' }}")
        self.assertEqual(lit("it's").bare(), "'it''s'")
        self.assertEqual(lit("both 'and' \"double\"").bare(), "'both ''and'' \"double\"'")

    def test_scalars_render_as_github_literals(self) -> None:
        self.assertEqual(lit(True).bare(), "true")
        self.assertEqual(lit(False).bare(), "false")
        self.assertEqual(lit(None).bare(), "null")
        self.assertEqual(lit(42).bare(), "42")
        self.assertEqual(lit(-7).bare(), "-7")

    def test_integral_floats_drop_the_trailing_zero(self) -> None:
        self.assertEqual(lit(5.0).bare(), "5")
        self.assertEqual(lit(1.2345).bare(), "1.2345")

    def test_non_finite_floats_are_rejected(self) -> None:
        for value in (math.nan, math.inf, -math.inf):
            with self.subTest(value=value):
                with self.assertRaises(LooptSDKError) as raised:
                    lit(value)
                self.assertEqual(raised.exception.code, "non_finite_number")

    def test_unsupported_literal_types_are_rejected(self) -> None:
        with self.assertRaises(TypeError):
            lit(["not", "a", "scalar"])

    def test_operand_passes_expressions_through(self) -> None:
        expression = github.ref
        self.assertIs(operand(expression), expression.expr)
        self.assertEqual(operand("x").bare(), "'x'")


class OperatorTests(unittest.TestCase):
    def test_comparisons_wrap_their_operands(self) -> None:
        self.assertEqual(
            eq(github.ref, "refs/heads/main").bare(),
            "github.ref == 'refs/heads/main'",
        )
        self.assertEqual(ne(github.ref, "main").bare(), "github.ref != 'main'")
        self.assertEqual(lt(github.run_number, 3).bare(), "github.run_number < 3")
        self.assertEqual(le(github.run_number, 3).bare(), "github.run_number <= 3")
        self.assertEqual(gt(github.run_number, 3).bare(), "github.run_number > 3")
        self.assertEqual(ge(github.run_number, 3).bare(), "github.run_number >= 3")

    def test_equality_parenthesizes_nested_equality(self) -> None:
        inner = eq(github.ref, "main")
        self.assertEqual(eq(inner, "x").bare(), "(github.ref == 'main') == 'x'")

    def test_logical_chain_flattens_equal_precedence(self) -> None:
        self.assertEqual(
            and_(eq(github.ref, "main"), eq(github.event_name, "push")).bare(),
            "github.ref == 'main' && github.event_name == 'push'",
        )

    def test_logical_chain_parenthesizes_looser_operands(self) -> None:
        self.assertEqual(
            and_(or_(github.a, github.b), github.c).bare(),
            "(github.a || github.b) && github.c",
        )
        self.assertEqual(
            or_(and_(github.a, github.b), github.c).bare(),
            "github.a && github.b || github.c",
        )

    def test_not_parenthesizes_binary_operands(self) -> None:
        self.assertEqual(not_(eq(github.ref, "main")).bare(), "!(github.ref == 'main')")
        self.assertEqual(not_(github.event_name).bare(), "!github.event_name")

    def test_member_and_index_access(self) -> None:
        self.assertEqual(at(github, "event").bare(), "github['event']")
        self.assertEqual(star(inputs.files).bare(), "inputs.files.*")
        self.assertEqual(
            at(github.event.pull_request, "number").bare(),
            "github.event.pull_request['number']",
        )

    def test_variadic_logical_operators_require_two_arguments(self) -> None:
        for builder, name in ((and_, "and"), (or_, "or")):
            with self.subTest(name=name):
                with self.assertRaises(LooptSDKError) as raised:
                    builder(github.ref)
                self.assertEqual(raised.exception.code, "invalid_arity")
                self.assertIn(name, str(raised.exception))


class FunctionTests(unittest.TestCase):
    def test_string_functions(self) -> None:
        self.assertEqual(
            contains(github.ref, "main").bare(),
            "contains(github.ref, 'main')",
        )
        self.assertEqual(
            starts_with(github.ref, "refs/heads/").bare(),
            "startsWith(github.ref, 'refs/heads/')",
        )
        self.assertEqual(
            ends_with(github.ref, "/main").bare(),
            "endsWith(github.ref, '/main')",
        )

    def test_format_and_join(self) -> None:
        self.assertEqual(
            format("hello {0}", github.actor).bare(),
            "format('hello {0}', github.actor)",
        )
        self.assertEqual(
            join(github.event.labels, ", ").bare(),
            "join(github.event.labels, ', ')",
        )
        self.assertEqual(join(github.event.labels).bare(), "join(github.event.labels)")

    def test_json_functions(self) -> None:
        self.assertEqual(to_json(github.event).bare(), "toJSON(github.event)")
        self.assertEqual(from_json(inputs.config).bare(), "fromJSON(inputs.config)")

    def test_hash_files_requires_patterns(self) -> None:
        self.assertEqual(
            hash_files("**/Cargo.lock", "**/Cargo.toml").bare(),
            "hashFiles('**/Cargo.lock', '**/Cargo.toml')",
        )
        with self.assertRaises(LooptSDKError) as raised:
            hash_files()
        self.assertEqual(raised.exception.code, "invalid_arity")

    def test_case_requires_an_odd_number_of_arguments(self) -> None:
        self.assertEqual(
            case_of(github.ref, "refs/heads/main", "release").bare(),
            "case(github.ref, 'refs/heads/main', 'release')",
        )
        with self.assertRaises(LooptSDKError) as raised:
            case_of(github.ref, "main", "release", "other")
        self.assertEqual(raised.exception.code, "invalid_arity")

    def test_status_functions_record_their_contexts(self) -> None:
        for build, context in (
            (success, "success"),
            (failure, "failure"),
            (always, "always"),
            (cancelled, "cancelled"),
        ):
            with self.subTest(context=context):
                expression = build()
                self.assertEqual(expression.bare(), f"{context}()")
                self.assertEqual(expression.contexts, frozenset({context}))


class ContextTests(unittest.TestCase):
    def test_nested_attribute_navigation(self) -> None:
        expression = github.event.pull_request.title
        self.assertEqual(expression.bare(), "github.event.pull_request.title")
        self.assertEqual(
            str(expression), "${{ github.event.pull_request.title }}"
        )

    def test_secret_names(self) -> None:
        expression = secrets.GITHUB_TOKEN
        self.assertEqual(expression.bare(), "secrets.GITHUB_TOKEN")
        self.assertEqual(expression.contexts, frozenset({"secrets"}))

    def test_matrix_axis_navigation(self) -> None:
        expression = matrix.os
        self.assertEqual(expression.bare(), "matrix.os")
        self.assertEqual(str(expression), "${{ matrix.os }}")
        self.assertEqual(matrix["os"].bare(), "matrix['os']")
        self.assertEqual(matrix.os.contexts, frozenset({"matrix"}))

    def test_item_navigation(self) -> None:
        self.assertEqual(github["event"].bare(), "github['event']")
        self.assertEqual(
            github.event.pull_request.at("title").bare(),
            "github.event.pull_request['title']",
        )
        self.assertEqual(
            github.event.pull_request[env.PR_FIELD].bare(),
            "github.event.pull_request[env.PR_FIELD]",
        )

    def test_camel_case_aliases_resolve_to_exact_keys(self) -> None:
        self.assertEqual(github.refName.bare(), "github.ref_name")
        self.assertEqual(strategy.failFast.bare(), "strategy.fail-fast")
        self.assertEqual(runner.toolCache.bare(), "runner.tool_cache")
        self.assertEqual(job_context.checkRunId.bare(), "job.check_run_id")

    def test_aliases_apply_only_at_the_root(self) -> None:
        self.assertEqual(github.event.refName.bare(), "github.event.refName")

    def test_root_contexts_record_themselves(self) -> None:
        for context, name in (
            (github, "github"),
            (env, "env"),
            (vars, "vars"),
            (secrets, "secrets"),
            (inputs, "inputs"),
            (runner, "runner"),
            (strategy, "strategy"),
            (matrix, "matrix"),
            (needs, "needs"),
            (steps, "steps"),
            (job_context, "job"),
        ):
            with self.subTest(name=name):
                self.assertEqual(context.contexts, frozenset({name}))

    def test_composed_expressions_merge_contexts(self) -> None:
        expression = and_(
            eq(github.ref, "refs/heads/main"),
            eq(secrets.DEPLOY_TOKEN, inputs.token),
        )
        self.assertEqual(expression.contexts, frozenset({"github", "secrets", "inputs"}))

    def test_raw_source_detects_contexts(self) -> None:
        expression = Expr.raw("github.event_name == 'push' && steps.build.outcome == 'success'")
        self.assertEqual(expression.contexts, frozenset({"github", "steps"}))
        # Detection is case-insensitive.
        self.assertEqual(
            Expr.raw("GitHub.ref == env['X']").contexts,
            frozenset({"github", "env"}),
        )
        # Identifiers inside single-quoted strings are not contexts.
        self.assertEqual(Expr.raw("'github.ref'").contexts, frozenset())
        # Contexts behind member access are not top-level contexts.
        self.assertEqual(Expr.raw("github['event'].name").contexts, frozenset({"github"}))


class ValidationTests(unittest.TestCase):
    def test_empty_source_is_rejected(self) -> None:
        for source in ("", "   ", "\n\t"):
            with self.subTest(source=source):
                with self.assertRaises(LooptSDKError) as raised:
                    Expr.raw(source)
                self.assertEqual(raised.exception.code, "empty_expression")

    def test_wrapper_delimiters_are_rejected(self) -> None:
        for source in ("${{ github.ref }}", "github.ref }}", "${{"):
            with self.subTest(source=source):
                with self.assertRaises(LooptSDKError) as raised:
                    Expr.raw(source)
                self.assertEqual(raised.exception.code, "forbidden_delimiter")

    def test_delimiters_inside_quoted_literals_are_allowed(self) -> None:
        self.assertEqual(
            Expr.raw("'${{ not a wrapper }}'").bare(),
            "'${{ not a wrapper }}'",
        )

    def test_non_finite_precedence_is_rejected(self) -> None:
        with self.assertRaises(LooptSDKError) as raised:
            Expr.with_precedence("github.ref", math.nan)
        self.assertEqual(raised.exception.code, "non_finite_precedence")


if __name__ == "__main__":
    unittest.main()
