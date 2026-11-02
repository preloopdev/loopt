"""Unit tests for :mod:`loopt_sdk.emit`."""

from __future__ import annotations

import contextlib
import io
import json
import unittest

from loopt_sdk.core import job, run, workflow
from loopt_sdk.emit import print_workflow
from loopt_sdk.expr import github, secrets


def capture(workflow_value: object) -> str:
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        print_workflow(workflow_value)
    return buffer.getvalue()


class EmitTests(unittest.TestCase):
    def test_prints_exactly_one_compact_json_line(self) -> None:
        stdout = capture({"name": "CI", "on": "push", "jobs": {"build": {}}})
        self.assertTrue(stdout.endswith("\n"))
        self.assertEqual(stdout.count("\n"), 1)
        self.assertEqual(
            stdout,
            '{"name":"CI","on":"push","jobs":{"build":{}}}\n',
        )

    def test_preserves_insertion_order(self) -> None:
        stdout = capture({"zebra": 1, "apple": 2, "middle": 3})
        self.assertLess(stdout.index('"zebra"'), stdout.index('"apple"'))
        self.assertLess(stdout.index('"apple"'), stdout.index('"middle"'))
        self.assertEqual(
            list(json.loads(stdout)),
            ["zebra", "apple", "middle"],
        )

    def test_unicode_is_written_literally(self) -> None:
        stdout = capture({"name": "Déploiement 🚀 — 日本語"})
        self.assertIn("Déploiement 🚀 — 日本語", stdout)
        self.assertNotIn("\\u", stdout)

    def test_scalars_keep_their_json_types(self) -> None:
        value = {
            "int": 7,
            "float": 5.0,
            "whole_float": 1.5,
            "bool": True,
            "null": None,
            "array": [1, "two"],
        }
        parsed = json.loads(capture(value))
        self.assertEqual(parsed, value)
        self.assertIsInstance(parsed["float"], float)
        self.assertIsInstance(parsed["int"], int)

    def test_embedded_expressions_render_as_wrapped_strings(self) -> None:
        parsed = json.loads(
            capture({"if": github.ref, "token": secrets.GITHUB_TOKEN})
        )
        self.assertEqual(
            parsed,
            {
                "if": "${{ github.ref }}",
                "token": "${{ secrets.GITHUB_TOKEN }}",
            },
        )

    def test_accepts_a_workflow_builder(self) -> None:
        wf = workflow().name("CI").on_("push")
        wf.job("build", job().runs_on("ubuntu-latest").step(run("make").build()).build())
        parsed = json.loads(capture(wf))
        self.assertEqual(parsed["name"], "CI")
        self.assertEqual(parsed["jobs"]["build"]["runs-on"], "ubuntu-latest")

    def test_rejects_non_workflow_values(self) -> None:
        with self.assertRaises(TypeError):
            capture(["not", "a", "workflow"])

    def test_rejects_non_finite_numbers(self) -> None:
        with self.assertRaises(ValueError):
            capture({"value": float("nan")})


if __name__ == "__main__":
    unittest.main()
