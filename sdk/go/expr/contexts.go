package expr

// Context is a GitHub Actions context expression that can be navigated
// dynamically: Field yields another Context rendered as `base.name` or
// `base['name']`, and At yields `base[key]` for a fully dynamic key.
type Context struct {
	expr    Expr
	aliases map[string]string
}

// NewContext builds a root context expression named name.
func NewContext(name string) (Context, error) {
	expr, err := rootContext(name)
	if err != nil {
		return Context{}, err
	}
	return Context{expr: expr}, nil
}

func mustContext(name string, aliases map[string]string) Context {
	context, err := NewContext(name)
	if err != nil {
		panic("expr: built-in context name is invalid: " + err.Error())
	}
	context.aliases = aliases
	return context
}

// Field builds `base.name` or `base['name']` at Member precedence,
// resolving name through this context's alias table first (e.g.
// Github().Field("refName") resolves to github.ref_name). The returned
// context carries no alias table of its own, matching the TypeScript SDK
// (github.event is a plain, alias-free context).
func (c Context) Field(name string) Context {
	resolved := name
	if alias, ok := c.aliases[name]; ok {
		resolved = alias
	}
	source := c.expr.source + "." + resolved
	if !isIdentifier(resolved) {
		source = c.expr.source + "[" + quote(resolved) + "]"
	}
	return Context{
		expr: composed(source, c.expr.contexts, PrecedenceMember),
	}
}

// At builds `base[key]` at Member precedence for a key that isn't a fixed
// field name (e.g. a loop variable).
func (c Context) At(key any) (Context, error) {
	keyExpr, err := Operand(key)
	if err != nil {
		return Context{}, err
	}
	contexts := make(map[string]struct{}, len(c.expr.contexts)+len(keyExpr.contexts))
	for name := range c.expr.contexts {
		contexts[name] = struct{}{}
	}
	for name := range keyExpr.contexts {
		contexts[name] = struct{}{}
	}
	return Context{
		expr: composed(c.expr.source+"["+keyExpr.source+"]", contexts, PrecedenceMember),
	}, nil
}

// Expr returns the context's underlying expression.
func (c Context) Expr() Expr {
	return c.expr
}

// Bare returns the bare expression text without the `${{ ... }}` wrapper.
func (c Context) Bare() string {
	return c.expr.Bare()
}

// String renders the wrapped form, `${{ source }}`.
func (c Context) String() string {
	return c.expr.String()
}

// isIdentifier matches property names GitHub accepts in dereference
// syntax.
func isIdentifier(name string) bool {
	chars := []rune(name)
	if len(chars) == 0 || !(isASCIIAlphabetic(chars[0]) || chars[0] == '_') {
		return false
	}
	for _, c := range chars[1:] {
		if !isASCIIAlphanumeric(c) && c != '_' && c != '-' {
			return false
		}
	}
	return true
}

// camelCase aliases for the snake_case members of the github context.
var githubAliases = map[string]string{
	"actionPath":        "action_path",
	"actionRef":         "action_ref",
	"actionRepository":  "action_repository",
	"actionStatus":      "action_status",
	"actorId":           "actor_id",
	"apiUrl":            "api_url",
	"artifactsList":     "artifacts_list",
	"baseRef":           "base_ref",
	"eventName":         "event_name",
	"eventPath":         "event_path",
	"graphqlUrl":        "graphql_url",
	"headRef":           "head_ref",
	"refName":           "ref_name",
	"refProtected":      "ref_protected",
	"refType":           "ref_type",
	"repositoryId":      "repository_id",
	"repositoryOwner":   "repository_owner",
	"repositoryOwnerId": "repository_owner_id",
	"repositoryUrl":     "repositoryUrl",
	"retentionDays":     "retention_days",
	"runAttempt":        "run_attempt",
	"runId":             "run_id",
	"runNumber":         "run_number",
	"secretSource":      "secret_source",
	"serverUrl":         "server_url",
	"triggeringActor":   "triggering_actor",
	"workflowRef":       "workflow_ref",
	"workflowSha":       "workflow_sha",
}

var runnerAliases = map[string]string{
	"toolCache": "tool_cache",
}

var strategyAliases = map[string]string{
	"failFast":    "fail-fast",
	"jobIndex":    "job-index",
	"jobTotal":    "job-total",
	"maxParallel": "max-parallel",
}

var jobAliases = map[string]string{
	"checkRunId":         "check_run_id",
	"workflowFilePath":   "workflow_file_path",
	"workflowRef":        "workflow_ref",
	"workflowRepository": "workflow_repository",
	"workflowSha":        "workflow_sha",
}

// Github returns the `github` context: information about the workflow run.
// Snake_case members are reachable through Field("ref_name") and through
// their camelCase aliases (Field("refName")); `event` stays a plain dynamic
// object.
func Github() Context {
	return mustContext("github", githubAliases)
}

// Env returns the `env` context: variables set in a workflow, job, or step.
func Env() Context {
	return mustContext("env", nil)
}

// Vars returns the `vars` context: configuration variables set at the
// repository, organization, or environment level.
func Vars() Context {
	return mustContext("vars", nil)
}

// Secrets returns the `secrets` context: names and values of secrets
// available to the run.
func Secrets() Context {
	return mustContext("secrets", nil)
}

// Inputs returns the `inputs` context: inputs of a reusable or manually
// triggered workflow.
func Inputs() Context {
	return mustContext("inputs", nil)
}

// Runner returns the `runner` context: information about the runner
// executing the job.
func Runner() Context {
	return mustContext("runner", runnerAliases)
}

// Strategy returns the `strategy` context: information about the current
// matrix strategy.
func Strategy() Context {
	return mustContext("strategy", strategyAliases)
}

// Job returns the `job` context: information about the currently running
// job. The name leaves `core.NewJob` free for the job builder constructor.
func Job() Context {
	return mustContext("job", jobAliases)
}

// Needs returns the `needs` context, used to build
// `needs.<id>.outputs.<name>` and `.result` job references.
func Needs() Context {
	return mustContext("needs", nil)
}

// Steps returns the `steps` context, used to build
// `steps.<id>.outputs.<name>`, `.outcome`, and `.conclusion` step
// references.
func Steps() Context {
	return mustContext("steps", nil)
}
