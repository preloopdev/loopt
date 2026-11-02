"""Workflow authoring core: ``run``/``uses`` steps, jobs, workflows, and
typed references to job and step outputs.

Builders mutate and return themselves so calls chain::

    build = (run("cargo build").id("build").build())
    ci = (
        job()
        .name("build")
        .runs_on("ubuntu-latest")
        .output("artifact", build.output("artifact"))
        .step(build)
        .build()
    )
    wf = workflow().name("CI").on_("push")
    job_ref = wf.job("build", ci)
    emit.print_workflow(wf.build())
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from typing import Any

from .errors import LooptSDKError
from .expr import Context, Expr, needs as _needs, steps as _steps


def _to_value(value: Any) -> Any:
    """Convert SDK expressions to their rendered strings, recursively."""
    if isinstance(value, (Expr, Context)):
        return str(value)
    if isinstance(value, Mapping):
        return {key: _to_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_to_value(item) for item in value]
    return value


def _condition(value: Expr | Context | str) -> str:
    """Render an ``if:`` condition, wrapping bare expression source."""
    if isinstance(value, (Expr, Context)):
        return str(value)
    return str(Expr.raw(str(value)))


def _job_ids(refs: Iterable["JobRef | str"]) -> list[str]:
    return [ref.id if isinstance(ref, JobRef) else str(ref) for ref in refs]


class JobRef:
    """A job's id plus the outputs it declared, for building ``needs`` refs."""

    __slots__ = ("_id", "_declared_outputs")

    def __init__(self, job_id: str, declared_outputs: list[str] | None) -> None:
        self._id = job_id
        self._declared_outputs = declared_outputs

    @property
    def id(self) -> str:
        """The id this job was placed under."""
        return self._id

    def output(self, name: str) -> Expr:
        """Build ``needs.<id>.outputs.<name>``."""
        if self._declared_outputs is not None and name not in self._declared_outputs:
            listed = (
                ", ".join(repr(output) for output in self._declared_outputs)
                if self._declared_outputs
                else "none"
            )
            raise LooptSDKError(
                "undeclared_output",
                f"job output {name!r} is not declared; declared outputs: {listed}",
            )
        return _needs.field(self._id).field("outputs").field(name).expr

    def result(self) -> Expr:
        """Build ``needs.<id>.result``."""
        return _needs.field(self._id).field("result").expr

    def __str__(self) -> str:
        return self._id

    def __repr__(self) -> str:
        return f"JobRef({self._id!r})"


class StepBuilder:
    """A step under construction, from :func:`run` or :func:`uses`."""

    def __init__(self, kind: str, target: str) -> None:
        self._kind = kind
        self._target = target
        self._id: str | None = None
        self._if: str | None = None
        self._name: str | None = None
        self._with: list[tuple[str, Any]] = []
        self._env: list[tuple[str, Any]] = []
        self._shell: str | None = None
        self._working_directory: str | None = None
        self._continue_on_error: bool | None = None
        self._timeout_minutes: int | None = None

    def id(self, step_id: str) -> "StepBuilder":
        """Set ``id:``, required to reference this step's outputs later."""
        self._id = str(step_id)
        return self

    def name(self, name: str) -> "StepBuilder":
        """Set ``name:``."""
        self._name = str(name)
        return self

    def if_cond(self, condition: Expr | Context | str) -> "StepBuilder":
        """Set ``if:``."""
        self._if = _condition(condition)
        return self

    def with_(self, key: str, value: Any) -> "StepBuilder":
        """Append one ``with:`` entry, preserving call order."""
        self._with.append((str(key), _to_value(value)))
        return self

    def env(self, key: str, value: Any) -> "StepBuilder":
        """Append one ``env:`` entry, preserving call order."""
        self._env.append((str(key), _to_value(value)))
        return self

    def shell(self, shell: str) -> "StepBuilder":
        """Set ``shell:``."""
        self._shell = str(shell)
        return self

    def working_directory(self, directory: str) -> "StepBuilder":
        """Set ``working-directory:``."""
        self._working_directory = str(directory)
        return self

    def continue_on_error(self, value: bool) -> "StepBuilder":
        """Set ``continue-on-error:``."""
        self._continue_on_error = bool(value)
        return self

    def timeout_minutes(self, minutes: int) -> "StepBuilder":
        """Set ``timeout-minutes:``."""
        self._timeout_minutes = int(minutes)
        return self

    def build(self) -> "StepRef":
        """Finish the step and return a typed reference to it."""
        value: dict[str, Any] = {}
        if self._id is not None:
            value["id"] = self._id
        if self._if is not None:
            value["if"] = self._if
        if self._name is not None:
            value["name"] = self._name
        value["uses" if self._kind == "uses" else "run"] = self._target
        if self._with:
            value["with"] = dict(self._with)
        if self._shell is not None:
            value["shell"] = self._shell
        if self._working_directory is not None:
            value["working-directory"] = self._working_directory
        if self._env:
            value["env"] = dict(self._env)
        if self._continue_on_error is not None:
            value["continue-on-error"] = self._continue_on_error
        if self._timeout_minutes is not None:
            value["timeout-minutes"] = self._timeout_minutes
        return StepRef(value, self._id)


class StepRef:
    """A finished step, plus typed access to its outputs and result."""

    __slots__ = ("_value", "_id")

    def __init__(self, value: dict[str, Any], step_id: str | None) -> None:
        self._value = value
        self._id = step_id

    @property
    def value(self) -> dict[str, Any]:
        """The step's JSON value."""
        return self._value

    @property
    def id(self) -> str | None:
        """The explicit ``id`` this step carries, if any."""
        return self._id

    def output(self, name: str) -> Expr:
        """Build ``steps.<id>.outputs.<name>``."""
        return _steps.field(self._require_id()).field("outputs").field(name).expr

    def outcome(self) -> Expr:
        """Build ``steps.<id>.outcome``."""
        return _steps.field(self._require_id()).field("outcome").expr

    def conclusion(self) -> Expr:
        """Build ``steps.<id>.conclusion``."""
        return _steps.field(self._require_id()).field("conclusion").expr

    def _require_id(self) -> str:
        if self._id is None:
            raise LooptSDKError(
                "missing_step_id",
                "this step has no `id`, so its outputs/outcome/conclusion "
                "cannot be referenced",
            )
        return self._id

    def __repr__(self) -> str:
        return f"StepRef({self._id!r})"


class Job:
    """A finished job, ready to place under a workflow's ``jobs:``."""

    __slots__ = ("_value", "_declared_outputs")

    def __init__(self, value: dict[str, Any], declared_outputs: list[str] | None) -> None:
        self._value = value
        self._declared_outputs = declared_outputs

    @property
    def value(self) -> dict[str, Any]:
        """The job's JSON value."""
        return self._value


class JobBuilder:
    """A regular job (``runs-on:``) under construction.

    Field order on :meth:`build` follows GitHub's conventional layout:
    ``name``, ``needs``, ``if``, ``runs-on``, ``strategy``,
    ``timeout-minutes``, ``concurrency``, ``env``, ``outputs``, ``steps``.
    """

    def __init__(self) -> None:
        self._name: str | None = None
        self._needs: list[str] = []
        self._if: str | None = None
        self._runs_on: Any = None
        self._strategy: Any = None
        self._timeout_minutes: int | None = None
        self._concurrency: Any = None
        self._env: list[tuple[str, Any]] = []
        self._outputs: list[tuple[str, Any]] = []
        self._steps: list[StepRef] = []

    def name(self, name: str) -> "JobBuilder":
        """Set ``name:``."""
        self._name = str(name)
        return self

    def needs(self, refs: Iterable[JobRef | str]) -> "JobBuilder":
        """Set ``needs:`` to the ids of ``refs``, in order."""
        self._needs = _job_ids(refs)
        return self

    def if_cond(self, condition: Expr | Context | str) -> "JobBuilder":
        """Set ``if:``."""
        self._if = _condition(condition)
        return self

    def runs_on(self, runner: Any) -> "JobBuilder":
        """Set ``runs-on:``."""
        self._runs_on = _to_value(runner)
        return self

    def strategy(self, strategy: Any) -> "JobBuilder":
        """Set ``strategy:`` (including its ``matrix:`` sub-key)."""
        self._strategy = _to_value(strategy)
        return self

    def timeout_minutes(self, minutes: int) -> "JobBuilder":
        """Set ``timeout-minutes:``."""
        self._timeout_minutes = int(minutes)
        return self

    def concurrency(self, concurrency: Any) -> "JobBuilder":
        """Set ``concurrency:`` (a group string or group/cancel mapping)."""
        self._concurrency = _to_value(concurrency)
        return self

    def env(self, key: str, value: Any) -> "JobBuilder":
        """Append one ``env:`` entry, preserving call order."""
        self._env.append((str(key), _to_value(value)))
        return self

    def output(self, name: str, value: Any) -> "JobBuilder":
        """Append one ``outputs:`` entry and declare the output name."""
        self._outputs.append((str(name), _to_value(value)))
        return self

    def step(self, step: StepRef) -> "JobBuilder":
        """Append one step to ``steps:``."""
        self._steps.append(step)
        return self

    def build(self) -> Job:
        """Finish the job."""
        value: dict[str, Any] = {}
        if self._name is not None:
            value["name"] = self._name
        if self._needs:
            value["needs"] = (
                self._needs[0] if len(self._needs) == 1 else list(self._needs)
            )
        if self._if is not None:
            value["if"] = self._if
        if self._runs_on is not None:
            value["runs-on"] = self._runs_on
        if self._strategy is not None:
            value["strategy"] = self._strategy
        if self._timeout_minutes is not None:
            value["timeout-minutes"] = self._timeout_minutes
        if self._concurrency is not None:
            value["concurrency"] = self._concurrency
        if self._env:
            value["env"] = dict(self._env)
        declared_outputs = [name for name, _ in self._outputs]
        if self._outputs:
            value["outputs"] = dict(self._outputs)
        value["steps"] = [step.value for step in self._steps]
        return Job(value, declared_outputs)


class CallJobBuilder:
    """A reusable-workflow caller job (``uses:``) under construction.

    Field order on :meth:`build` follows GitHub's conventional layout:
    ``name``, ``needs``, ``if``, ``uses``, ``with``, ``secrets``,
    ``strategy``, ``concurrency``.
    """

    def __init__(self, uses: str) -> None:
        self._name: str | None = None
        self._needs: list[str] = []
        self._if: str | None = None
        self._uses = str(uses)
        self._with: list[tuple[str, Any]] = []
        self._secrets: list[tuple[str, Any]] = []
        self._strategy: Any = None
        self._concurrency: Any = None

    def name(self, name: str) -> "CallJobBuilder":
        """Set ``name:``."""
        self._name = str(name)
        return self

    def needs(self, refs: Iterable[JobRef | str]) -> "CallJobBuilder":
        """Set ``needs:`` to the ids of ``refs``, in order."""
        self._needs = _job_ids(refs)
        return self

    def if_cond(self, condition: Expr | Context | str) -> "CallJobBuilder":
        """Set ``if:``."""
        self._if = _condition(condition)
        return self

    def with_(self, key: str, value: Any) -> "CallJobBuilder":
        """Append one ``with:`` entry, preserving call order."""
        self._with.append((str(key), _to_value(value)))
        return self

    def secrets(self, key: str, value: Any) -> "CallJobBuilder":
        """Append one ``secrets:`` entry, preserving call order."""
        self._secrets.append((str(key), _to_value(value)))
        return self

    def strategy(self, strategy: Any) -> "CallJobBuilder":
        """Set ``strategy:`` (including its ``matrix:`` sub-key)."""
        self._strategy = _to_value(strategy)
        return self

    def concurrency(self, concurrency: Any) -> "CallJobBuilder":
        """Set ``concurrency:`` (a group string or group/cancel mapping)."""
        self._concurrency = _to_value(concurrency)
        return self

    def build(self) -> Job:
        """Finish the job."""
        value: dict[str, Any] = {}
        if self._name is not None:
            value["name"] = self._name
        if self._needs:
            value["needs"] = (
                self._needs[0] if len(self._needs) == 1 else list(self._needs)
            )
        if self._if is not None:
            value["if"] = self._if
        value["uses"] = self._uses
        if self._with:
            value["with"] = dict(self._with)
        if self._secrets:
            value["secrets"] = dict(self._secrets)
        if self._strategy is not None:
            value["strategy"] = self._strategy
        if self._concurrency is not None:
            value["concurrency"] = self._concurrency
        return Job(value, None)


class WorkflowBuilder:
    """A whole workflow document under construction."""

    def __init__(self) -> None:
        self._name: str | None = None
        self._on: Any = None
        self._permissions: Any = None
        self._env: list[tuple[str, Any]] = []
        self._concurrency: Any = None
        self._defaults: Any = None
        self._jobs: dict[str, Any] = {}

    def name(self, name: str) -> "WorkflowBuilder":
        """Set ``name:``."""
        self._name = str(name)
        return self

    def on_(self, on: Any) -> "WorkflowBuilder":
        """Set ``on:``."""
        self._on = _to_value(on)
        return self

    def permissions(self, permissions: Any) -> "WorkflowBuilder":
        """Set ``permissions:``."""
        self._permissions = _to_value(permissions)
        return self

    def env(self, key: str, value: Any) -> "WorkflowBuilder":
        """Append one ``env:`` entry, preserving call order."""
        self._env.append((str(key), _to_value(value)))
        return self

    def concurrency(self, concurrency: Any) -> "WorkflowBuilder":
        """Set ``concurrency:``."""
        self._concurrency = _to_value(concurrency)
        return self

    def defaults(self, defaults: Any) -> "WorkflowBuilder":
        """Set ``defaults:``."""
        self._defaults = _to_value(defaults)
        return self

    def job(self, job_id: str, job: Job) -> JobRef:
        """Place ``job`` under ``jobs.<job_id>`` and return its reference."""
        self._jobs[str(job_id)] = job.value
        return JobRef(str(job_id), job._declared_outputs)

    def build(self) -> dict[str, Any]:
        """Finish the workflow document."""
        value: dict[str, Any] = {}
        if self._name is not None:
            value["name"] = self._name
        if self._on is not None:
            value["on"] = self._on
        if self._permissions is not None:
            value["permissions"] = self._permissions
        if self._env:
            value["env"] = dict(self._env)
        if self._concurrency is not None:
            value["concurrency"] = self._concurrency
        if self._defaults is not None:
            value["defaults"] = self._defaults
        value["jobs"] = self._jobs
        return value


def run(command: str) -> StepBuilder:
    """Build a step that runs a shell command (``run:``)."""
    return StepBuilder("run", str(command))


def uses(action: str) -> StepBuilder:
    """Build a step that runs an action (``uses:``)."""
    return StepBuilder("uses", str(action))


def job() -> JobBuilder:
    """Build a regular job (``runs-on:``)."""
    return JobBuilder()


def call_workflow(uses: str) -> CallJobBuilder:
    """Build a reusable-workflow caller job (``uses:``)."""
    return CallJobBuilder(uses)


def workflow() -> WorkflowBuilder:
    """Build a workflow document."""
    return WorkflowBuilder()
