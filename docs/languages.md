# Languages and plugins

Loopt authors workflows in any language. TypeScript, Python, Go, and Rust
ship as first-party (built-in) languages; everything else plugs in through
the plugin protocol — no Loopt changes required.

There are two extension paths: an out-of-tree plugin (no Loopt changes), or a
built-in language in this repository.

## Out-of-tree plugin

A plugin is any executable that reads one JSON request on stdin, writes one
JSON response on stdout, and exits. Loopt runs it with the repository root as
its working directory; it never goes through a shell. `--plugin` names the
program and each `--plugin-arg` is passed as a separate argument.

Protocol v1 requests:

```json
{"version":1,"op":"describe"}
{"version":1,"op":"emit","params":{"workflow":{"name":"ci"},"stem":".loopt/workflows/ci"}}
{"version":1,"op":"synth","params":{"source":".loopt/workflows/ci.jsonwf"}}
```

`describe` reports `{"language":"jsonwf","extensions":["jsonwf"]}`. `emit`
returns `{"entrypoint":".loopt/workflows/ci.jsonwf","files":[{"path":".loopt/workflows/ci.jsonwf","content":"..."}]}`;
every path must stay under `.loopt/workflows` and must include `entrypoint`.
`synth` runs or reads the source itself and returns
`{"workflow":{...}}`. Wrap successful results as
`{"status":"ok","version":1,"result":...}`; return
`{"status":"error","version":1,"message":"..."}` (and optionally a non-zero
exit) on failure. A protocol-version mismatch is a hard error.

A minimal JSON round-trip plugin:

```python
#!/usr/bin/env python3
import json, pathlib, sys

request = json.load(sys.stdin)
op = request["op"]

def ok(result):
    print(json.dumps({"status": "ok", "version": 1, "result": result}))

if op == "describe":
    ok({"language": "jsonwf", "extensions": ["jsonwf"]})
elif op == "emit":
    stem = request["params"]["stem"]
    path = f"{stem}.jsonwf"
    content = json.dumps(request["params"]["workflow"], indent=2) + "\n"
    ok({"entrypoint": path, "files": [{"path": path, "content": content}]})
elif op == "synth":
    source = pathlib.Path(request["params"]["source"]).read_text()
    ok({"workflow": json.loads(source)})
else:
    print(json.dumps({"status": "error", "version": 1,
                      "message": f"unknown op {op}"}))
    sys.exit(1)
```

Wire it in:

```sh
chmod +x bin/loopt-jsonwf
loopt import --language jsonwf --plugin ./bin/loopt-jsonwf
loopt gen
```

If the plugin needs arguments, use repeatable `--plugin-arg`:

```sh
loopt import --language jsonwf --plugin python3 \
  --plugin-arg ./bin/loopt-jsonwf.py
```

`loopt gen` reuses the plugin recorded in `.loopt/manifest.json`. Hand-authored
sources in a custom language need a manifest entry; run `loopt import` once or
add the `language` and `plugin` fields to the entry.

### Java plugin example

Java needs no wrapper script and no Loopt change: pass `java` as the plugin
program and the plugin source as its argument. `examples/java/JavaPlugin.java`
is a complete JDK-only plugin (JDK 11+):

```sh
loopt import --language java --plugin java \
  --plugin-arg examples/java/JavaPlugin.java
loopt gen
```

It emits a `.java` entrypoint that prints the workflow JSON, and `synth` runs
it with `java <source>`. The example deliberately keeps the workflow as an
embedded JSON string; a production built-in Java emitter would generate
ordinary Java data structures and a JSON writer instead.

## Built-in language

Add `src/languages/<name>.rs` implementing:

- `emit(workflow, stem) -> GeneratedSource` — produce the editable source file(s).
- `command(source) -> CommandSpec` — how to execute the entrypoint from the repo root.

Then wire the module into `src/languages/mod.rs`: add it to `Language::ALL` and
extend `id`, `parse`, `source_extensions`, `detect_entrypoint`, `emit`, and
`command`. Add round-trip tests under `tests/`. The hidden
`loopt plugin <language>` subcommand exercises a built-in through the same
request/response protocol as an external plugin, which makes it useful as a
reference while implementing one.
