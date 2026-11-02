"""GitHub Actions expression builders.

Expressions render as ``${{ source }}`` when stringified and expose the bare
source through :meth:`Expr.bare`. Contexts are module-level objects that can
be navigated with ordinary attribute or item access::

    github.event.pull_request.title   # -> ${{ github.event.pull_request.title }}
    secrets.GITHUB_TOKEN              # -> ${{ secrets.GITHUB_TOKEN }}
    github.event["action"]            # -> ${{ github.event['action'] }}

Operators and built-in functions return new :class:`Expr` values and
parenthesize their operands only where the surrounding precedence requires
it, matching the Rust and TypeScript SDKs.
"""

from __future__ import annotations

import math
from typing import Any, Iterable, Union

from .errors import LooptSDKError

# Operator precedence levels, ordered from the loosest binding (OR) to the
# tightest (ATOM), matching GitHub Actions expression grammar.
OR = 1.0
AND = 2.0
EQUALITY = 3.0
COMPARISON = 4.0
NOT = 5.0
MEMBER = 6.0
ATOM = 7.0

_WHITESPACE = " \t\n\r"


def _quote(value: str) -> str:
    """Render ``value`` as a single-quoted expression string literal."""
    return "'" + value.replace("'", "''") + "'"


def _format_float(value: float) -> str:
    """Render a finite float the way GitHub expression literals expect.

    Integral values lose the trailing ``.0`` (matching the Rust SDK's
    ``format!("{value}")``); everything else uses Python's shortest
    round-tripping ``repr``.
    """
    if not math.isfinite(value):
        raise LooptSDKError(
            "non_finite_number",
            "cannot render a non-finite number as an expression literal",
        )
    if value.is_integer() and abs(value) < 1e21:
        if value == 0.0 and math.copysign(1.0, value) < 0:
            return "-0"
        return str(int(value))
    return repr(value)


def _find_forbidden_delimiter(source: str) -> str | None:
    """Return the first ``${{`` or ``}}`` outside a quoted literal, if any."""
    index = 0
    length = len(source)
    in_string = False
    while index < length:
        char = source[index]
        if in_string:
            if char == "'":
                if index + 1 < length and source[index + 1] == "'":
                    index += 2
                else:
                    in_string = False
                    index += 1
            else:
                index += 1
            continue
        if char == "'":
            in_string = True
            index += 1
            continue
        if source.startswith("${{", index):
            return "${{"
        if source.startswith("}}", index):
            return "}}"
        index += 1
    return None


def _is_name_char(char: str) -> bool:
    return char.isascii() and (char.isalnum() or char in "_-")


def _previous_significant(source: str, index: int) -> str | None:
    """Return the closest non-whitespace character before ``index``."""
    cursor = index - 1
    while cursor >= 0:
        if source[cursor] not in _WHITESPACE:
            return source[cursor]
        cursor -= 1
    return None


def _detect_contexts(source: str) -> set[str]:
    """Collect the lowercase top-level contexts referenced by ``source``.

    A top-level context is an identifier followed (after optional
    whitespace) by ``.`` or ``[`` that is not itself the tail of a member
    access. Identifiers inside single-quoted literals and function names are
    ignored.
    """
    contexts: set[str] = set()
    index = 0
    length = len(source)
    in_string = False
    while index < length:
        char = source[index]
        if in_string:
            if char == "'":
                if index + 1 < length and source[index + 1] == "'":
                    index += 2
                else:
                    in_string = False
                    index += 1
            else:
                index += 1
            continue
        if char == "'":
            in_string = True
            index += 1
            continue
        if not (char.isascii() and (char.isalpha() or char == "_")):
            index += 1
            continue

        start = index
        end = index + 1
        while end < length and _is_name_char(source[end]):
            end += 1
        lookahead = end
        while lookahead < length and source[lookahead] in _WHITESPACE:
            lookahead += 1
        following = source[lookahead] if lookahead < length else None
        previous = _previous_significant(source, start)
        if following in (".", "[") and previous not in (".", "[", "'"):
            contexts.add(source[start:end].lower())
        index = end
    return contexts


def _is_identifier(name: str) -> bool:
    """Whether GitHub accepts ``name`` in dotted dereference syntax."""
    if not name or not (name[0].isascii() and (name[0].isalpha() or name[0] == "_")):
        return False
    return all(_is_name_char(char) for char in name[1:])


class Expr:
    """A GitHub Actions expression: its source, contexts and precedence."""

    __slots__ = ("_source", "_contexts", "_precedence")

    def __init__(
        self,
        source: str,
        contexts: Iterable[str] = (),
        precedence: float = ATOM,
    ) -> None:
        self._source = source
        self._contexts = frozenset(name.lower() for name in contexts)
        self._precedence = precedence

    @classmethod
    def raw(cls, source: str) -> "Expr":
        """Wrap ``source`` at atom precedence, recording referenced contexts.

        Raises :class:`LooptSDKError` when ``source`` is empty or contains a
        ``${{``/``}}`` wrapper delimiter outside a single-quoted literal.
        """
        return cls._validated(str(source), (), ATOM)

    @classmethod
    def with_precedence(cls, source: str, precedence: float) -> "Expr":
        """As :meth:`raw`, but at an explicit operator precedence."""
        if not math.isfinite(precedence):
            raise LooptSDKError(
                "non_finite_precedence",
                "expression precedence must be a finite number",
            )
        return cls._validated(str(source), (), precedence)

    @classmethod
    def _validated(
        cls,
        source: str,
        extra_contexts: Iterable[str],
        precedence: float,
    ) -> "Expr":
        if not source.strip():
            raise LooptSDKError(
                "empty_expression",
                "expression source must not be empty or whitespace",
            )
        forbidden = _find_forbidden_delimiter(source)
        if forbidden is not None:
            raise LooptSDKError(
                "forbidden_delimiter",
                "expression source must not contain the wrapper delimiter "
                f"{forbidden!r} outside a single-quoted string literal",
            )
        if not math.isfinite(precedence):
            raise LooptSDKError(
                "non_finite_precedence",
                "expression precedence must be a finite number",
            )
        contexts = _detect_contexts(source)
        contexts.update(name.lower() for name in extra_contexts)
        return cls(source, contexts, precedence)

    @classmethod
    def _composed(
        cls,
        source: str,
        contexts: Iterable[str],
        precedence: float,
    ) -> "Expr":
        return cls(source, contexts, precedence)

    @property
    def source(self) -> str:
        """The bare expression text."""
        return self._source

    @property
    def contexts(self) -> frozenset[str]:
        """Lowercase names of the top-level contexts referenced."""
        return self._contexts

    @property
    def precedence(self) -> float:
        """Operator precedence, used when composing larger expressions."""
        return self._precedence

    def bare(self) -> str:
        """The expression text without the ``${{ ... }}`` wrapper."""
        return self._source

    def field(self, name: str) -> "Context":
        """Navigate to ``.<name>`` (or ``['<name>']``) at member precedence."""
        source = (
            f"{self._source}.{name}"
            if _is_identifier(name)
            else f"{self._source}[{_quote(name)}]"
        )
        return Context._from_expr(
            Expr._composed(source, self._contexts, MEMBER)
        )

    def at(self, key: "Operand") -> "Context":
        """Index into this expression with a dynamic key."""
        key_expr = operand(key)
        contexts = set(self._contexts) | key_expr.contexts
        return Context._from_expr(
            Expr._composed(f"{self._source}[{key_expr.source}]", contexts, MEMBER)
        )

    def __getattr__(self, name: str) -> "Context":
        if name.startswith("_"):
            raise AttributeError(name)
        return self.field(name)

    def __getitem__(self, key: "Operand") -> "Context":
        return self.at(key)

    def __str__(self) -> str:
        return f"${{{{ {self._source} }}}}"

    def __repr__(self) -> str:
        return f"Expr({self._source!r})"


AliasTable = dict[str, str]


class Context:
    """A navigable context expression such as ``github`` or ``secrets``.

    Attribute access yields the next member (``github.event``), item access
    yields ``base[key]`` (``github.event['action']``). Root contexts may
    carry camelCase aliases that resolve to GitHub's exact keys.
    """

    __slots__ = ("_expr", "_aliases")

    def __init__(self, name: str, aliases: AliasTable | None = None) -> None:
        base = Expr.raw(name)
        self._expr = Expr._composed(
            base.source,
            frozenset({name.lower()}) | base.contexts,
            base.precedence,
        )
        self._aliases = dict(aliases or {})

    @classmethod
    def _from_expr(cls, expr: Expr) -> "Context":
        context = object.__new__(cls)
        context._expr = expr
        context._aliases = {}
        return context

    @property
    def expr(self) -> Expr:
        """The underlying expression."""
        return self._expr

    @property
    def source(self) -> str:
        return self._expr.source

    @property
    def contexts(self) -> frozenset[str]:
        return self._expr.contexts

    @property
    def precedence(self) -> float:
        return self._expr.precedence

    def bare(self) -> str:
        return self._expr.bare()

    def field(self, name: str) -> "Context":
        resolved = self._aliases.get(name, name)
        if _is_identifier(resolved):
            source = f"{self._expr.source}.{resolved}"
        else:
            source = f"{self._expr.source}[{_quote(resolved)}]"
        return Context._from_expr(
            Expr._composed(source, self._expr.contexts, MEMBER)
        )

    def at(self, key: "Operand") -> "Context":
        key_expr = operand(key)
        contexts = set(self._expr.contexts) | key_expr.contexts
        return Context._from_expr(
            Expr._composed(
                f"{self._expr.source}[{key_expr.source}]",
                contexts,
                MEMBER,
            )
        )

    def __getattr__(self, name: str) -> "Context":
        if name.startswith("_"):
            raise AttributeError(name)
        return self.field(name)

    def __getitem__(self, key: "Operand") -> "Context":
        return self.at(key)

    def __str__(self) -> str:
        return str(self._expr)

    def __repr__(self) -> str:
        return f"Context({self._expr.source!r})"


Operand = Union[Expr, Context, str, int, float, bool, None]


def lit(value: str | int | float | bool | None) -> Expr:
    """Render a JSON scalar as an expression literal."""
    if value is None:
        source = "null"
    elif isinstance(value, bool):
        source = "true" if value else "false"
    elif isinstance(value, int):
        source = str(value)
    elif isinstance(value, float):
        source = _format_float(value)
    elif isinstance(value, str):
        source = _quote(value)
    else:
        raise TypeError(
            "lit() accepts str, int, float, bool, or None; "
            f"received {type(value).__name__}"
        )
    return Expr._composed(source, (), ATOM)


def operand(value: Operand) -> Expr:
    """Coerce an expression or literal into an :class:`Expr`."""
    if isinstance(value, Context):
        return value.expr
    if isinstance(value, Expr):
        return value
    return lit(value)


def _merge_contexts(*exprs: Expr) -> frozenset[str]:
    contexts: set[str] = set()
    for expr in exprs:
        contexts.update(expr.contexts)
    return frozenset(contexts)


def _below(expr: Expr, limit: float) -> Expr:
    """Parenthesize ``expr`` when it binds less tightly than ``limit``."""
    if expr.precedence >= limit:
        return expr
    return Expr._composed(f"({expr.source})", expr.contexts, ATOM)


def _at_most(expr: Expr, limit: float) -> Expr:
    """Parenthesize ``expr`` when it binds no more tightly than ``limit``."""
    if expr.precedence > limit:
        return expr
    return Expr._composed(f"({expr.source})", expr.contexts, ATOM)


def _comparison(symbol: str, limit: float, left: Operand, right: Operand) -> Expr:
    left_expr = _at_most(operand(left), limit)
    right_expr = _at_most(operand(right), limit)
    return Expr._composed(
        f"{left_expr.source} {symbol} {right_expr.source}",
        _merge_contexts(left_expr, right_expr),
        limit,
    )


def eq(left: Operand, right: Operand) -> Expr:
    """Build ``left == right``."""
    return _comparison("==", EQUALITY, left, right)


def ne(left: Operand, right: Operand) -> Expr:
    """Build ``left != right``."""
    return _comparison("!=", EQUALITY, left, right)


def lt(left: Operand, right: Operand) -> Expr:
    """Build ``left < right``."""
    return _comparison("<", COMPARISON, left, right)


def le(left: Operand, right: Operand) -> Expr:
    """Build ``left <= right``."""
    return _comparison("<=", COMPARISON, left, right)


def gt(left: Operand, right: Operand) -> Expr:
    """Build ``left > right``."""
    return _comparison(">", COMPARISON, left, right)


def ge(left: Operand, right: Operand) -> Expr:
    """Build ``left >= right``."""
    return _comparison(">=", COMPARISON, left, right)


def _logical(name: str, symbol: str, limit: float, values: Iterable[Operand]) -> Expr:
    arguments = tuple(values)
    if len(arguments) < 2:
        raise LooptSDKError(
            "invalid_arity",
            f"{name}() expects at least 2 arguments; received {len(arguments)}",
        )
    rendered = [_below(operand(value), limit) for value in arguments]
    return Expr._composed(
        f" {symbol} ".join(expr.source for expr in rendered),
        _merge_contexts(*rendered),
        limit,
    )


def and_(*values: Operand) -> Expr:
    """Build ``a && b && ...``."""
    return _logical("and", "&&", AND, values)


def or_(*values: Operand) -> Expr:
    """Build ``a || b || ...``."""
    return _logical("or", "||", OR, values)


def not_(value: Operand) -> Expr:
    """Build ``!value``."""
    inner = _below(operand(value), NOT)
    return Expr._composed(f"!{inner.source}", inner.contexts, NOT)


def at(obj: Operand, key: Operand) -> Expr:
    """Build ``obj[key]`` at member precedence."""
    base = _below(operand(obj), MEMBER)
    key_expr = operand(key)
    return Expr._composed(
        f"{base.source}[{key_expr.source}]",
        _merge_contexts(base, key_expr),
        MEMBER,
    )


def star(obj: Operand) -> Expr:
    """Build ``obj.*`` (object filter access)."""
    base = _below(operand(obj), MEMBER)
    return Expr._composed(f"{base.source}.*", base.contexts, MEMBER)


def _call(name: str, args: Iterable[Operand], extra_contexts: Iterable[str] = ()) -> Expr:
    rendered = [operand(arg) for arg in args]
    contexts: set[str] = {context.lower() for context in extra_contexts}
    for expr in rendered:
        contexts.update(expr.contexts)
    return Expr._composed(
        f"{name}({', '.join(expr.source for expr in rendered)})",
        contexts,
        ATOM,
    )


def contains(search: Operand, item: Operand) -> Expr:
    """Build ``contains(search, item)``."""
    return _call("contains", (search, item))


def starts_with(value: Operand, prefix: Operand) -> Expr:
    """Build ``startsWith(value, prefix)``."""
    return _call("startsWith", (value, prefix))


def ends_with(value: Operand, suffix: Operand) -> Expr:
    """Build ``endsWith(value, suffix)``."""
    return _call("endsWith", (value, suffix))


def format(template: Operand, *values: Operand) -> Expr:  # noqa: A001
    """Build ``format(template, ...values)``."""
    return _call("format", (template, *values))


def join(array: Operand, separator: Operand | None = None) -> Expr:
    """Build ``join(array, separator?)``."""
    args: tuple[Operand, ...] = (array,) if separator is None else (array, separator)
    return _call("join", args)


def to_json(value: Operand) -> Expr:
    """Build ``toJSON(value)``."""
    return _call("toJSON", (value,))


def from_json(value: Operand) -> Expr:
    """Build ``fromJSON(value)``."""
    return _call("fromJSON", (value,))


def hash_files(*patterns: str) -> Expr:
    """Build ``hashFiles(...patterns)`` with 1 to 255 patterns."""
    if not 1 <= len(patterns) <= 255:
        raise LooptSDKError(
            "invalid_arity",
            f"hashFiles() expects 1 to 255 patterns; received {len(patterns)}",
        )
    return _call("hashFiles", patterns, ("hashfiles",))


def case_of(*args: Operand) -> Expr:
    """Build ``case(value, predicate, result, ...)`` with an odd argument count."""
    if len(args) < 3 or len(args) % 2 == 0:
        raise LooptSDKError(
            "invalid_arity",
            "case() expects an odd number of at least 3 arguments; "
            f"received {len(args)}",
        )
    return _call("case", args)


def _status(name: str) -> Expr:
    return Expr._composed(f"{name}()", (name.lower(),), ATOM)


def success() -> Expr:
    """Build ``success()``."""
    return _status("success")


def failure() -> Expr:
    """Build ``failure()``."""
    return _status("failure")


def always() -> Expr:
    """Build ``always()``."""
    return _status("always")


def cancelled() -> Expr:
    """Build ``cancelled()``."""
    return _status("cancelled")


# camelCase aliases for the snake_case members of the `github` context.
_GITHUB_ALIASES: AliasTable = {
    "actionPath": "action_path",
    "actionRef": "action_ref",
    "actionRepository": "action_repository",
    "actionStatus": "action_status",
    "actorId": "actor_id",
    "apiUrl": "api_url",
    "artifactsList": "artifacts_list",
    "baseRef": "base_ref",
    "eventName": "event_name",
    "eventPath": "event_path",
    "graphqlUrl": "graphql_url",
    "headRef": "head_ref",
    "refName": "ref_name",
    "refProtected": "ref_protected",
    "refType": "ref_type",
    "repositoryId": "repository_id",
    "repositoryOwner": "repository_owner",
    "repositoryOwnerId": "repository_owner_id",
    "repositoryUrl": "repositoryUrl",
    "retentionDays": "retention_days",
    "runAttempt": "run_attempt",
    "runId": "run_id",
    "runNumber": "run_number",
    "secretSource": "secret_source",
    "serverUrl": "server_url",
    "triggeringActor": "triggering_actor",
    "workflowRef": "workflow_ref",
    "workflowSha": "workflow_sha",
}

_RUNNER_ALIASES: AliasTable = {"toolCache": "tool_cache"}

_STRATEGY_ALIASES: AliasTable = {
    "failFast": "fail-fast",
    "jobIndex": "job-index",
    "jobTotal": "job-total",
    "maxParallel": "max-parallel",
}

_JOB_ALIASES: AliasTable = {
    "checkRunId": "check_run_id",
    "workflowFilePath": "workflow_file_path",
    "workflowRef": "workflow_ref",
    "workflowRepository": "workflow_repository",
    "workflowSha": "workflow_sha",
}

github = Context("github", _GITHUB_ALIASES)
env = Context("env")
vars = Context("vars")
secrets = Context("secrets")
inputs = Context("inputs")
runner = Context("runner", _RUNNER_ALIASES)
strategy = Context("strategy", _STRATEGY_ALIASES)
needs = Context("needs")
steps = Context("steps")
job_context = Context("job", _JOB_ALIASES)
# `job` names the job context inside this module; the package root exposes
# `loopt_sdk.job` as the job-builder constructor instead.
job = job_context
