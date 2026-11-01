import { context } from "../expr/index.ts";
import type { DynExpr } from "../expr/index.ts";
import { obj } from "../kit/index.ts";
import type { DocValue, Expr } from "../kit/index.ts";

/** Optional `include` and `exclude` filters for a build matrix. */
export interface MatrixOptions {
  /** Extra combinations appended to the matrix, as a document or expression. */
  include?: DocValue | Expr;
  /** Combinations removed from the matrix, as a document or expression. */
  exclude?: DocValue | Expr;
}

/**
 * A build matrix definition produced by {@link matrix} or
 * {@link MatrixFactory.fromJson}.
 *
 * Reading any other property returns the matching `matrix.<axis>` expression.
 */
export class MatrixRef {
  /** The `strategy.matrix` document, or the `fromJson` expression. */
  readonly definition: DocValue;

  /** Axis names in declaration order, used to describe the matrix. */
  readonly axes: readonly string[];

  constructor(definition: DocValue, axes: readonly string[]) {
    this.definition = definition;
    this.axes = axes;
  }
}

/** Returns the document (or expression) to place at `strategy.matrix`. */
export function matrixDefinition(m: MatrixRef): DocValue {
  return m.definition;
}

/** Root context used to render axis reads such as `${{ matrix.os }}`. */
const MATRIX_CONTEXT: DynExpr = context("matrix");

/** Real object members that must not be interpreted as matrix axes. */
const RESERVED_MEMBERS: ReadonlySet<string> = new Set([
  "axes",
  "constructor",
  "definition",
  "then",
  "toJSON",
  "toString",
  "valueOf",
]);

/**
 * Wraps `ref` so that reading an unknown string property yields the matching
 * `matrix.<axis>` expression.
 */
function wrap(ref: MatrixRef): MatrixRef & { readonly [axis: string]: DynExpr } {
  const handler: ProxyHandler<MatrixRef> = {
    get(target, property, receiver) {
      if (typeof property === "symbol") {
        return Reflect.get(target, property, receiver);
      }
      const name = String(property);
      if (RESERVED_MEMBERS.has(name)) {
        return Reflect.get(target, name, receiver);
      }
      return MATRIX_CONTEXT[name];
    },
  };
  return new Proxy(ref, handler) as MatrixRef & { readonly [axis: string]: DynExpr };
}

/** Builds the definition for a matrix built from explicit axes. */
function axisDefinition(
  axes: Record<string, DocValue | Expr>,
  options?: MatrixOptions,
): MatrixRef & { readonly [axis: string]: DynExpr } {
  const entries: Array<[string, DocValue]> = [];
  const names: string[] = [];
  for (const [name, value] of Object.entries(axes)) {
    names.push(name);
    entries.push([name, value]);
  }
  if (options?.include !== undefined) {
    entries.push(["include", options.include]);
  }
  if (options?.exclude !== undefined) {
    entries.push(["exclude", options.exclude]);
  }
  return wrap(new MatrixRef(obj(entries), names));
}

/** Builds a matrix whose definition is the given expression. */
function jsonDefinition<Shape extends Record<string, unknown>>(
  expr: Expr<string>,
): MatrixRef & { readonly [K in keyof Shape]: DynExpr } {
  const ref = new MatrixRef(expr, []);
  return wrap(ref) as unknown as MatrixRef & { readonly [K in keyof Shape]: DynExpr };
}

/** Callable surface of {@link matrix}. */
export interface MatrixFactory {
  /**
   * Builds a matrix from named axes and optional `include`/`exclude` filters,
   * to be used as `strategy.matrix`.
   */
  (
    axes: Record<string, DocValue | Expr>,
    options?: MatrixOptions,
  ): MatrixRef & { readonly [axis: string]: DynExpr };

  /**
   * Builds a matrix whose whole definition comes from an expression, such as
   * the result of `fromJSON(...)`.
   */
  fromJson<Shape extends Record<string, unknown>>(
    expr: Expr<string>,
  ): MatrixRef & { readonly [K in keyof Shape]: DynExpr };
}

/**
 * Builds a build-matrix definition for `strategy.matrix`.
 *
 * Axes are stored first in their own order, followed by `include` and
 * `exclude` when supplied. Axis reads produce `${{ matrix.<axis> }}`.
 */
export const matrix: MatrixFactory = Object.assign(axisDefinition, {
  fromJson: jsonDefinition,
});
