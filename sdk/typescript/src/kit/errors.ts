/**
 * Error type shared by the whole kit. Every failure raised by the kit is a
 * {@link LooptError} carrying a stable machine-readable `code` so callers can
 * branch without parsing messages.
 */
export class LooptError extends Error {
  /** Stable machine-readable error code, e.g. `"invalid-expression"`. */
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = "LooptError";
    this.code = code;
  }
}
