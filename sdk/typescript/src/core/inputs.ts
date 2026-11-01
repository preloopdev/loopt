/**
 * Options shared by every workflow input definition.
 */
export interface InputOptions {
  /** Human-readable description of the input. */
  description?: string;
  /** Whether callers must supply the input. */
  required?: boolean;
  /** Value used when the input is not supplied. */
  default?: string | number | boolean;
}

/** A `string` input definition, suitable for dispatch and reusable-workflow inputs. */
export interface StringInputDefinition extends InputOptions {
  type: "string";
}

/** A `number` input definition, suitable for dispatch and reusable-workflow inputs. */
export interface NumberInputDefinition extends InputOptions {
  type: "number";
}

/** A `boolean` input definition, suitable for dispatch and reusable-workflow inputs. */
export interface BooleanInputDefinition extends InputOptions {
  type: "boolean";
}

/** Options accepted by {@link InputFactory.choice}. */
export interface ChoiceOptions extends InputOptions {
  /** Allowed choices, in display order. */
  options: string[];
}

/** A `choice` input definition, for `workflow_dispatch` inputs. */
export interface ChoiceInputDefinition extends InputOptions {
  type: "choice";
  /** Allowed choices, in display order. */
  options: string[];
}

/** An `environment` input definition, for `workflow_dispatch` inputs. */
export interface EnvironmentInputDefinition extends InputOptions {
  type: "environment";
}

/** Callable surface of {@link input}. */
export interface InputFactory {
  /** Defines a `string` input. */
  string(options?: InputOptions): StringInputDefinition;
  /** Defines a `number` input. */
  number(options?: InputOptions): NumberInputDefinition;
  /** Defines a `boolean` input. */
  boolean(options?: InputOptions): BooleanInputDefinition;
  /** Defines a `choice` input with a fixed set of options. */
  choice(options: ChoiceOptions): ChoiceInputDefinition;
  /** Defines an `environment` input. */
  environment(options?: InputOptions): EnvironmentInputDefinition;
}

/**
 * Builds `workflow_dispatch` and `workflow_call` input definitions.
 *
 * Definitions use the canonical key order `description`, `type`, `required`,
 * `default`, `options`, matching the generated schema.
 */
export const input: InputFactory = {
  string(options?: InputOptions): StringInputDefinition {
    return {
      ...(options?.description !== undefined ? { description: options.description } : {}),
      type: "string",
      ...(options?.required !== undefined ? { required: options.required } : {}),
      ...(options?.default !== undefined ? { default: options.default } : {}),
    };
  },

  number(options?: InputOptions): NumberInputDefinition {
    return {
      ...(options?.description !== undefined ? { description: options.description } : {}),
      type: "number",
      ...(options?.required !== undefined ? { required: options.required } : {}),
      ...(options?.default !== undefined ? { default: options.default } : {}),
    };
  },

  boolean(options?: InputOptions): BooleanInputDefinition {
    return {
      ...(options?.description !== undefined ? { description: options.description } : {}),
      type: "boolean",
      ...(options?.required !== undefined ? { required: options.required } : {}),
      ...(options?.default !== undefined ? { default: options.default } : {}),
    };
  },

  choice(options: ChoiceOptions): ChoiceInputDefinition {
    return {
      ...(options.description !== undefined ? { description: options.description } : {}),
      type: "choice",
      ...(options.required !== undefined ? { required: options.required } : {}),
      ...(options.default !== undefined ? { default: options.default } : {}),
      options: [...options.options],
    };
  },

  environment(options?: InputOptions): EnvironmentInputDefinition {
    return {
      ...(options?.description !== undefined ? { description: options.description } : {}),
      type: "environment",
      ...(options?.required !== undefined ? { required: options.required } : {}),
      ...(options?.default !== undefined ? { default: options.default } : {}),
    };
  },
};
