"""Writes an assembled workflow document to stdout.

``loopt gen`` reads exactly one JSON object from a source program's stdout;
this module is the single place the SDK produces that shape.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping
from typing import Any

from .expr import Context, Expr


def _json_ready(value: Any) -> Any:
    """Replace SDK expression objects with their rendered strings."""
    if isinstance(value, (Expr, Context)):
        return str(value)
    if isinstance(value, Mapping):
        return {key: _json_ready(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_ready(item) for item in value]
    return value


def print_workflow(workflow: Any) -> None:
    """Print ``workflow`` as one compact JSON line on stdout.

    Accepts a finished document (a mapping) or a
    :class:`loopt_sdk.core.WorkflowBuilder`, and renders any embedded
    expression objects as their ``${{ ... }}`` strings.
    """
    build = getattr(workflow, "build", None)
    if callable(build):
        workflow = build()
    if not isinstance(workflow, Mapping):
        raise TypeError(
            "print_workflow() expects a workflow mapping or a "
            f"WorkflowBuilder, received {type(workflow).__name__}"
        )

    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except AttributeError:
        pass

    rendered = json.dumps(
        _json_ready(workflow),
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
    )
    sys.stdout.write(rendered)
    sys.stdout.write("\n")
