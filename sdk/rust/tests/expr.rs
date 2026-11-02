//! Expression rendering, precedence, literal, operator, function, and
//! context-navigation tests for `loopt-sdk`'s `expr` module.

use loopt_sdk::expr::{
    always, and, at, cancelled, case_of, contains, ends_with, eq, failure, format, from_json, ge,
    github, gt, hash_files, inputs, job_context, join, le, lit, lt, matrix, ne, not, operand, or,
    runner, secrets, star, starts_with, strategy, success, to_json, vars, Expr, Literal, Operand,
};
use loopt_sdk::Error;

#[test]
fn display_wraps_and_bare_does_not() {
    let expr = Expr::raw("github.ref").expect("valid source");
    assert_eq!(expr.to_string(), "${{ github.ref }}");
    assert_eq!(expr.bare(), "github.ref");
}

#[test]
fn raw_rejects_empty_and_whitespace_sources() {
    assert_eq!(Expr::raw("").unwrap_err(), Error::EmptyExpression);
    assert_eq!(Expr::raw("   \t\n").unwrap_err(), Error::EmptyExpression);
}

#[test]
fn raw_rejects_forbidden_delimiters_outside_quotes() {
    assert!(matches!(
        Expr::raw("a ${{ b }}").unwrap_err(),
        Error::ForbiddenDelimiter("${{")
    ));
    assert!(matches!(
        Expr::raw("a }} b").unwrap_err(),
        Error::ForbiddenDelimiter("}}")
    ));
    // Inside a single-quoted literal, the delimiters are inert.
    assert!(Expr::raw("'a ${{ b }} c'").is_ok());
    // A doubled '' escapes a single quote without ending the literal.
    assert!(Expr::raw("'it''s ${{ fine }}'").is_ok());
}

#[test]
fn raw_rejects_non_finite_precedence() {
    assert_eq!(
        Expr::with_precedence("x", f64::NAN).unwrap_err(),
        Error::NonFinitePrecedence
    );
    assert_eq!(
        Expr::with_precedence("x", f64::INFINITY).unwrap_err(),
        Error::NonFinitePrecedence
    );
}

#[test]
fn raw_detects_top_level_contexts_case_insensitively() {
    let expr = Expr::raw("GitHub.ref == env['X']").expect("valid source");
    assert!(expr.contexts().contains("github"));
    assert!(expr.contexts().contains("env"));
    assert_eq!(expr.contexts().len(), 2);
}

#[test]
fn lit_quotes_strings_and_escapes_embedded_quotes() {
    let expr = lit("it's fine").expect("string literal");
    assert_eq!(expr.bare(), "'it''s fine'");
}

#[test]
fn lit_renders_numbers_bools_and_null() {
    assert_eq!(lit(5i64).unwrap().bare(), "5");
    assert_eq!(lit(5.0f64).unwrap().bare(), "5");
    assert_eq!(lit(3.5f64).unwrap().bare(), "3.5");
    assert_eq!(lit(true).unwrap().bare(), "true");
    assert_eq!(lit(false).unwrap().bare(), "false");
    assert_eq!(lit(Literal::Null).unwrap().bare(), "null");
}

#[test]
fn lit_rejects_non_finite_numbers() {
    assert_eq!(lit(f64::NAN).unwrap_err(), Error::NonFiniteNumber);
    assert_eq!(lit(f64::INFINITY).unwrap_err(), Error::NonFiniteNumber);
}

#[test]
fn operand_passes_expressions_through_and_lowers_literals() {
    let expr = Expr::raw("github.ref").unwrap();
    assert_eq!(operand(expr.clone()).unwrap().bare(), "github.ref");
    assert_eq!(operand("x").unwrap().bare(), "'x'");
    assert_eq!(operand(Operand::from(7i64)).unwrap().bare(), "7");
}

#[test]
fn comparisons_render_with_equality_and_comparison_precedence() {
    assert_eq!(
        eq(github().field("ref"), "refs/heads/main").unwrap().bare(),
        "github.ref == 'refs/heads/main'"
    );
    assert_eq!(ne(1i64, 2i64).unwrap().bare(), "1 != 2");
    assert_eq!(lt(1i64, 2i64).unwrap().bare(), "1 < 2");
    assert_eq!(le(1i64, 2i64).unwrap().bare(), "1 <= 2");
    assert_eq!(gt(1i64, 2i64).unwrap().bare(), "1 > 2");
    assert_eq!(ge(1i64, 2i64).unwrap().bare(), "1 >= 2");
}

#[test]
fn and_or_not_parenthesize_lower_precedence_operands() {
    let left = eq(1i64, 1i64).unwrap();
    let right = eq(2i64, 2i64).unwrap();
    let conjunction = and(&[left.clone().into(), right.clone().into()]).unwrap();
    assert_eq!(conjunction.bare(), "1 == 1 && 2 == 2");

    let disjunction = or(&[left.into(), right.into()]).unwrap();
    assert_eq!(disjunction.bare(), "1 == 1 || 2 == 2");

    // `&&` must parenthesize an operand that binds looser than And (an
    // `||` chain does); nothing binds looser than `||` itself, so `or()`
    // never needs to wrap one of its own operands.
    let weak = Expr::with_precedence("a || b", loopt_sdk::expr::precedence::OR).unwrap();
    let wrapped = and(&[weak.into(), lit(true).unwrap().into()]).unwrap();
    assert_eq!(wrapped.bare(), "(a || b) && true");

    assert_eq!(not(true).unwrap().bare(), "!true");
}

#[test]
fn and_or_require_at_least_two_values() {
    let err = and(&[true.into()]).unwrap_err();
    assert!(matches!(
        err,
        Error::InvalidArity {
            name: "and",
            received: 1,
            ..
        }
    ));
    let err = or(&[]).unwrap_err();
    assert!(matches!(
        err,
        Error::InvalidArity {
            name: "or",
            received: 0,
            ..
        }
    ));
}

#[test]
fn at_and_star_render_member_precedence_access() {
    assert_eq!(
        at(github().field("event"), "ref").unwrap().bare(),
        "github.event['ref']"
    );
    assert_eq!(
        star(github().field("event")).unwrap().bare(),
        "github.event.*"
    );
}

#[test]
fn functions_render_calls_and_record_context_sensitive_names() {
    assert_eq!(contains("abc", "b").unwrap().bare(), "contains('abc', 'b')");
    assert_eq!(
        starts_with("abc", "a").unwrap().bare(),
        "startsWith('abc', 'a')"
    );
    assert_eq!(
        ends_with("abc", "c").unwrap().bare(),
        "endsWith('abc', 'c')"
    );
    assert_eq!(
        format("{0}-{1}", &["a".into(), "b".into()]).unwrap().bare(),
        "format('{0}-{1}', 'a', 'b')"
    );
    assert_eq!(join("items", None).unwrap().bare(), "join('items')");
    assert_eq!(
        join("items", Some("separator".into())).unwrap().bare(),
        "join('items', 'separator')"
    );
    assert_eq!(to_json("x").unwrap().bare(), "toJSON('x')");
}

#[test]
fn from_json_renders_call() {
    assert_eq!(from_json("x").unwrap().bare(), "fromJSON('x')");
}

#[test]
fn hash_files_records_hashfiles_context_and_checks_arity() {
    let expr = hash_files(&["**/*.lock"]).unwrap();
    assert_eq!(expr.bare(), "hashFiles('**/*.lock')");
    assert!(expr.contexts().contains("hashfiles"));

    let err = hash_files(&[]).unwrap_err();
    assert!(matches!(
        err,
        Error::InvalidArity {
            name: "hashFiles",
            received: 0,
            ..
        }
    ));
}

#[test]
fn case_of_requires_odd_arity_of_at_least_three() {
    let expr = case_of(&[true.into(), "a".into(), "b".into()]).unwrap();
    assert_eq!(expr.bare(), "case(true, 'a', 'b')");

    let err = case_of(&[true.into(), "a".into()]).unwrap_err();
    assert!(matches!(
        err,
        Error::InvalidArity {
            name: "case",
            received: 2,
            ..
        }
    ));
}

#[test]
fn status_functions_render_zero_arg_calls_and_record_their_own_context() {
    assert_eq!(success().bare(), "success()");
    assert!(success().contexts().contains("success"));
    assert_eq!(failure().bare(), "failure()");
    assert_eq!(always().bare(), "always()");
    assert_eq!(cancelled().bare(), "cancelled()");
}

#[test]
fn context_field_renders_identifier_or_bracket_access() {
    let g = github();
    assert_eq!(g.field("ref").bare(), "github.ref");
    // A field name GitHub wouldn't accept as a bare identifier falls back
    // to bracket/quote syntax.
    assert_eq!(g.field("weird name").bare(), "github['weird name']");
}

#[test]
fn context_field_resolves_camel_case_aliases_for_every_builtin_table() {
    assert_eq!(github().field("refName").bare(), "github.ref_name");
    assert_eq!(github().field("actorId").bare(), "github.actor_id");
    assert_eq!(runner().field("toolCache").bare(), "runner.tool_cache");
    assert_eq!(strategy().field("failFast").bare(), "strategy.fail-fast");
    assert_eq!(strategy().field("jobIndex").bare(), "strategy.job-index");
    assert_eq!(job_context().field("checkRunId").bare(), "job.check_run_id");
    assert_eq!(
        job_context().field("workflowFilePath").bare(),
        "job.workflow_file_path"
    );
}

#[test]
fn context_field_passes_through_unaliased_and_nested_access() {
    // `event` has no alias; nested access keeps composing.
    let pr_title = github().field("event").field("pull_request").field("title");
    assert_eq!(pr_title.bare(), "github.event.pull_request.title");
    assert!(pr_title.expr().contexts().contains("github"));
}

#[test]
fn matrix_context_renders_user_defined_axes() {
    let m = matrix();
    assert_eq!(m.field("os").bare(), "matrix.os");
    // GitHub's own docs access hyphenated axes bare: `${{ matrix.node-version }}`.
    assert_eq!(m.field("node-version").bare(), "matrix.node-version");
    assert_eq!(m.field("os").expr().to_string(), "${{ matrix.os }}");
    assert!(m.field("os").expr().contexts().contains("matrix"));
}

#[test]
fn context_at_renders_dynamic_bracket_access_and_merges_contexts() {
    let index = Expr::raw("matrix.i").expect("valid source");
    let indexed = github().field("event").at(index).expect("valid key");
    assert_eq!(indexed.bare(), "github.event[matrix.i]");
    assert!(indexed.expr().contexts().contains("github"));
    assert!(indexed.expr().contexts().contains("matrix"));
}

#[test]
fn root_contexts_render_their_own_name_and_record_it() {
    for ctx in [vars(), secrets(), inputs()] {
        let name = ctx.bare();
        assert_eq!(ctx.expr().contexts().len(), 1);
        assert!(ctx.expr().contexts().contains(&name));
    }
}
