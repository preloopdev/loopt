"""Unit tests for :mod:`loopt_sdk.core`."""

from __future__ import annotations

import unittest

from loopt_sdk.core import (
    call_workflow,
    job,
    run,
    uses,
    workflow,
)
from loopt_sdk.errors import LooptSDKError
from loopt_sdk.expr import always, eq, github, secrets


class StepTests(unittest.TestCase):
    def test_run_step_orders_fields_conventionally(self) -> None:
        step = (
            run("cargo build")
            .id("build")
            .if_cond(always())
            .name("Build")
            .with_("profile", "release")
            .shell("bash")
            .working_directory("crate")
            .env("RUST_LOG", "debug")
            .continue_on_error(True)
            .timeout_minutes(10)
            .build()
        )
        self.assertEqual(
            list(step.value),
            [
                "id",
                "if",
                "name",
                "run",
                "with",
                "shell",
                "working-directory",
                "env",
                "continue-on-error",
                "timeout-minutes",
            ],
        )
        self.assertEqual(step.value["run"], "cargo build")
        self.assertEqual(step.value["with"], {"profile": "release"})
        self.assertEqual(step.value["env"], {"RUST_LOG": "debug"})
        self.assertEqual(step.value["if"], "${{ always() }}")
        self.assertEqual(step.value["timeout-minutes"], 10)
        self.assertEqual(step.id, "build")

    def test_uses_step_builds_action_inputs(self) -> None:
        step = uses("actions/checkout@v4").with_("fetch-depth", 0).build()
        self.assertEqual(
            step.value,
            {"uses": "actions/checkout@v4", "with": {"fetch-depth": 0}},
        )

    def test_expressions_are_rendered_inside_values(self) -> None:
        step = (
            uses("actions/cache@v4")
            .with_("key", github.ref)
            .env("TOKEN", secrets.GITHUB_TOKEN)
            .build()
        )
        self.assertEqual(step.value["with"]["key"], "${{ github.ref }}")
        self.assertEqual(step.value["env"]["TOKEN"], "${{ secrets.GITHUB_TOKEN }}")

    def test_step_outputs_render_steps_references(self) -> None:
        step = run("make").id("build").build()
        self.assertEqual(step.output("artifact").bare(), "steps.build.outputs.artifact")
        self.assertEqual(step.outcome().bare(), "steps.build.outcome")
        self.assertEqual(step.conclusion().bare(), "steps.build.conclusion")
        self.assertEqual(
            str(step.output("artifact")),
            "${{ steps.build.outputs.artifact }}",
        )

    def test_missing_step_id_cannot_be_referenced(self) -> None:
        step = run("make").build()
        self.assertIsNone(step.id)
        for build in (lambda: step.output("artifact"), step.outcome, step.conclusion):
            with self.subTest(build=build):
                with self.assertRaises(LooptSDKError) as raised:
                    build()
                self.assertEqual(raised.exception.code, "missing_step_id")


class JobTests(unittest.TestCase):
    def test_job_orders_fields_and_steps(self) -> None:
        build = run("make").id("build").build()
        test = run("make test").id("test").build()
        value = (
            job()
            .name("build")
            .runs_on("ubuntu-latest")
            .strategy({"matrix": {"os": ["ubuntu-latest", "macos-latest"]}})
            .timeout_minutes(30)
            .env("CI", "true")
            .output("artifact", build.output("artifact"))
            .step(build)
            .step(test)
            .build()
        )
        self.assertEqual(
            list(value.value),
            [
                "name",
                "runs-on",
                "strategy",
                "timeout-minutes",
                "env",
                "outputs",
                "steps",
            ],
        )
        self.assertEqual(
            value.value["outputs"],
            {"artifact": "${{ steps.build.outputs.artifact }}"},
        )
        self.assertEqual(
            [step["id"] for step in value.value["steps"]], ["build", "test"]
        )

    def test_concurrency_sits_before_strategy_and_timeout_minutes(self) -> None:
        value = (
            job()
            .runs_on("ubuntu-latest")
            .timeout_minutes(15)
            .concurrency(
                {"group": "ci-${{ github.ref }}", "cancel-in-progress": True}
            )
            .env("CI", "true")
            .build()
        )
        self.assertEqual(
            list(value.value),
            ["runs-on", "concurrency", "timeout-minutes", "env", "steps"],
        )
        self.assertEqual(
            value.value["concurrency"],
            {"group": "ci-${{ github.ref }}", "cancel-in-progress": True},
        )

    def test_job_emits_the_full_conventional_key_order(self) -> None:
        value = (
            job()
            .name("build")
            .needs(["lint"])
            .if_cond(github.ref)
            .runs_on("ubuntu-latest")
            .permissions({"contents": "read"})
            .environment({"name": "production", "url": "https://example.com"})
            .concurrency({"group": "ci", "cancel-in-progress": False})
            .container("node:20")
            .services({"redis": {"image": "redis:7"}})
            .defaults({"run": {"shell": "bash"}})
            .continue_on_error(True)
            .strategy({"matrix": {"os": ["ubuntu-latest"]}})
            .timeout_minutes(10)
            .env("CI", "true")
            .output("artifact", "dist.zip")
            .step(run("make").id("build").build())
            .build()
        )
        self.assertEqual(
            list(value.value),
            [
                "name",
                "needs",
                "if",
                "runs-on",
                "permissions",
                "environment",
                "concurrency",
                "container",
                "services",
                "defaults",
                "continue-on-error",
                "strategy",
                "timeout-minutes",
                "env",
                "outputs",
                "steps",
            ],
        )
        self.assertEqual(value.value["permissions"], {"contents": "read"})
        self.assertEqual(
            value.value["environment"],
            {"name": "production", "url": "https://example.com"},
        )
        self.assertEqual(value.value["container"], "node:20")
        self.assertEqual(value.value["services"], {"redis": {"image": "redis:7"}})
        self.assertEqual(value.value["defaults"], {"run": {"shell": "bash"}})
        self.assertIs(value.value["continue-on-error"], True)

    def test_environment_accepts_a_name_string(self) -> None:
        value = job().runs_on("ubuntu-latest").environment("production").build()
        self.assertEqual(value.value["environment"], "production")

    def test_needs_is_a_string_for_one_job_and_a_list_for_many(self) -> None:
        wf = workflow()
        first = wf.job("first", job().runs_on("ubuntu-latest").build())
        second = wf.job(
            "second", job().needs([first]).runs_on("ubuntu-latest").build()
        )
        third = wf.job(
            "third",
            job().needs([first, second]).runs_on("ubuntu-latest").build(),
        )
        jobs = wf.build()["jobs"]
        self.assertEqual(jobs["second"]["needs"], "first")
        self.assertEqual(jobs["third"]["needs"], ["first", "second"])

    def test_job_outputs_must_be_declared(self) -> None:
        wf = workflow()
        ref = wf.job(
            "build",
            job()
            .runs_on("ubuntu-latest")
            .output("artifact", "dist.zip")
            .build(),
        )
        self.assertEqual(ref.output("artifact").bare(), "needs.build.outputs.artifact")
        self.assertEqual(ref.result().bare(), "needs.build.result")
        with self.assertRaises(LooptSDKError) as raised:
            ref.output("missing")
        self.assertEqual(raised.exception.code, "undeclared_output")
        self.assertIn("artifact", str(raised.exception))


class CallWorkflowTests(unittest.TestCase):
    def test_caller_job_fields_and_secrets(self) -> None:
        other = job().runs_on("ubuntu-latest").build()
        wf = workflow()
        needs = wf.job("build", other)
        value = (
            call_workflow("./.github/workflows/reusable.yml")
            .name("reuse")
            .needs([needs])
            .if_cond(eq(github.ref, "refs/heads/main"))
            .with_("target", "production")
            .secrets("token", secrets.DEPLOY_TOKEN)
            .strategy({"matrix": {"target": ["staging", "production"]}})
            .build()
        )
        self.assertEqual(
            list(value.value),
            ["name", "needs", "if", "uses", "with", "secrets", "strategy"],
        )
        self.assertEqual(value.value["needs"], "build")
        self.assertEqual(value.value["uses"], "./.github/workflows/reusable.yml")
        self.assertEqual(
            value.value["with"], {"target": "production"}
        )
        self.assertEqual(
            value.value["secrets"], {"token": "${{ secrets.DEPLOY_TOKEN }}"}
        )

    def test_caller_concurrency_is_emitted_after_strategy(self) -> None:
        value = (
            call_workflow("./.github/workflows/reusable.yml")
            .name("reuse")
            .with_("target", "production")
            .secrets("token", secrets.DEPLOY_TOKEN)
            .strategy({"matrix": {"target": ["staging", "production"]}})
            .concurrency(
                {"group": "reuse-${{ github.ref }}", "cancel-in-progress": False}
            )
            .build()
        )
        self.assertEqual(
            list(value.value),
            ["name", "uses", "with", "secrets", "strategy", "concurrency"],
        )
        self.assertEqual(
            value.value["concurrency"],
            {"group": "reuse-${{ github.ref }}", "cancel-in-progress": False},
        )

    def test_caller_permissions_sit_after_if_before_uses(self) -> None:
        value = (
            call_workflow("./.github/workflows/reusable.yml")
            .name("reuse")
            .if_cond(eq(github.ref, "refs/heads/main"))
            .permissions({"contents": "read", "id-token": "write"})
            .with_("target", "production")
            .strategy({"matrix": {"target": ["production"]}})
            .concurrency("reuse")
            .build()
        )
        self.assertEqual(
            list(value.value),
            [
                "name",
                "if",
                "permissions",
                "uses",
                "with",
                "strategy",
                "concurrency",
            ],
        )
        self.assertEqual(
            value.value["permissions"],
            {"contents": "read", "id-token": "write"},
        )

    def test_caller_secrets_inherit_emits_the_string(self) -> None:
        value = (
            call_workflow("./.github/workflows/reusable.yml")
            .with_("target", "production")
            .secrets_inherit()
            .build()
        )
        self.assertEqual(
            list(value.value), ["uses", "with", "secrets"]
        )
        self.assertEqual(value.value["secrets"], "inherit")

    def test_caller_secrets_and_inherit_together_fail_loudly(self) -> None:
        builder = (
            call_workflow("./.github/workflows/reusable.yml")
            .secrets("token", secrets.DEPLOY_TOKEN)
            .secrets_inherit()
        )
        with self.assertRaises(LooptSDKError) as raised:
            builder.build()
        self.assertEqual(raised.exception.code, "conflicting_secrets")
        self.assertIn("secrets_inherit", str(raised.exception))

    def test_caller_outputs_are_not_validated(self) -> None:
        wf = workflow()
        ref = wf.job(
            "reuse",
            call_workflow("./.github/workflows/reusable.yml").build(),
        )
        self.assertEqual(ref.output("anything").bare(), "needs.reuse.outputs.anything")


class WorkflowTests(unittest.TestCase):
    def test_workflow_orders_fields_and_preserves_job_order(self) -> None:
        wf = (
            workflow()
            .name("CI")
            .on_({"push": {"branches": ["main"]}, "pull_request": {}})
            .permissions({"contents": "read"})
            .env("CI", "true")
            .concurrency("ci-${{ github.ref }}")
            .defaults({"run": {"shell": "bash"}})
        )
        wf.job("build", job().runs_on("ubuntu-latest").build())
        lint = wf.job("lint", job().runs_on("ubuntu-latest").build())
        wf.job(
            "test",
            job().needs([lint]).runs_on("ubuntu-latest").build(),
        )

        document = wf.build()
        self.assertEqual(
            list(document),
            ["name", "on", "permissions", "env", "concurrency", "defaults", "jobs"],
        )
        self.assertEqual(list(document["jobs"]), ["build", "lint", "test"])
        self.assertEqual(
            document["on"],
            {"push": {"branches": ["main"]}, "pull_request": {}},
        )

    def test_run_name_sits_right_after_name(self) -> None:
        wf = (
            workflow()
            .name("CI")
            .run_name("CI-${{ github.ref }}")
            .on_("push")
            .permissions({"contents": "read"})
            .env("CI", "true")
            .concurrency("ci")
            .defaults({"run": {"shell": "bash"}})
        )
        wf.job("build", job().runs_on("ubuntu-latest").build())

        document = wf.build()
        self.assertEqual(
            list(document),
            [
                "name",
                "run-name",
                "on",
                "permissions",
                "env",
                "concurrency",
                "defaults",
                "jobs",
            ],
        )
        self.assertEqual(document["run-name"], "CI-${{ github.ref }}")

    def test_job_expression_is_rendered_in_concurrency(self) -> None:
        wf = workflow().concurrency(github.ref)
        self.assertEqual(wf.build()["concurrency"], "${{ github.ref }}")


if __name__ == "__main__":
    unittest.main()
