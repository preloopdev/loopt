"""The error type shared by the whole ``loopt_sdk`` package.

Every failure the SDK raises is one :class:`LooptSDKError` carrying a stable
``code`` string, mirroring the machine-checkable failure modes of the Rust
and TypeScript SDKs.
"""

from __future__ import annotations


class LooptSDKError(ValueError):
    """An invalid expression or workflow-authoring reference.

    ``code`` is one of ``empty_expression``, ``forbidden_delimiter``,
    ``non_finite_precedence``, ``non_finite_number``, ``invalid_arity``,
    ``undeclared_output``, ``missing_step_id`` or ``conflicting_secrets``.
    """

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code

    def __repr__(self) -> str:
        return f"LooptSDKError({self.code!r}, {str(self)!r})"
