# ronten

A concern-based review viewer for agent-generated changes. An agent decomposes its own
diff into "concerns", a human reviews each concern in the browser, and the verdicts come
back to the agent as machine-readable JSON.

## Why

Reviewing a large agent-generated diff hunk-by-hunk is tedious and loses the "why". ronten
flips that: the agent proposes a small set of concerns (what changed and why), and the
diff itself is never agent-supplied — ronten reads the changed files and their blobs
directly via git plumbing (`rev-parse`, `merge-base`, `diff-tree --raw`, `cat-file`) and
computes the text diff with its own diff engine. Nothing in `.gitattributes` (`-diff`,
diff drivers, textconv), `diff.*` config, or `GIT_EXTERNAL_DIFF` can alter or hide what
is displayed. Any hunk that
doesn't map to a concern the agent proposed is never silently dropped; it is placed into an
auto-generated, warning-styled `_unmapped` concern that still requires a verdict from the
human before submission. The agent then reads the result JSON — including line-anchored
comments — straight into its fix loop.

ronten is daemonless: one review session is one process. It starts an HTTP server bound to
`127.0.0.1`, serves the review UI, waits for a submission (or abort, or timeout), prints the
result JSON to stdout, and exits. No background service, no state files, no shared ledger —
parallel worktrees can run simultaneous sessions on their own ports without conflict.

## Install

```sh
cargo install ronten
```

The published crate ships the prebuilt frontend, so `cargo install ronten` needs only a
Rust toolchain — no Node.js. Building from a **git checkout** requires **Node.js >= 20**
at build time: the embedded frontend (Svelte, built to static assets) is compiled by
`cargo build` via a build script and embedded into the binary. No Node.js is required
at runtime either way.

### From source

```sh
git clone https://github.com/t09tanaka/ronten
cd ronten
cargo build --release
./target/release/ronten demo
```

## Quick start

```sh
ronten demo
```

Launches the UI with an embedded sample diff and concerns — no git repository required.
Useful for a first look or for generating screenshots/GIFs.

## Agent integration

### `ronten review` contract

```sh
ronten review --base <ref> --concerns <file|-> [options]
```

| Option | Default | Description |
|---|---|---|
| `--base <ref>` | required | Comparison base; diffs `<ref>...HEAD` (merge-base semantics) |
| `--concerns <path>` | required | Concerns JSON path; `-` for stdin |
| `--out <path>` | none | Also write result JSON to a file (in addition to stdout) |
| `--port <n>` | `0` (OS-assigned) | Bind port, for fixed allocation (e.g. portool) |
| `--no-open` | false | Do not auto-open the browser; print URL only |
| `--title <s>` | branch name | Session display name |
| `--timeout <dur>` | none | Exit 3 if no submission within the duration (e.g. `30m`) |

**Output separation is strict**: human-facing logs (e.g.
`Review session: http://127.0.0.1:PORT/r/TOKEN`) go to stderr. Machine-readable data — the
result JSON — goes to stdout only, and only stdout. An agent can always do
`result=$(ronten review ...)` and safely `jq` the output.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | all concerns approved |
| 1 | one or more request-changes |
| 2 | reviewer aborted |
| 3 | timeout |
| 10 | invalid usage or invalid concerns JSON |
| 11 | base ref not resolvable |
| 12 | not a git repository |
| 13 | empty diff (nothing to review) |
| 14 | git invocation failed |

### `ronten schema`

```sh
ronten schema             # both schemas
ronten schema --input     # concerns JSON Schema only
ronten schema --output    # result JSON Schema only
```

Prints the JSON Schemas for the concerns input and result output to stdout. The schemas are
generated from the same serde types the binary uses at runtime, so they can never drift from
the implementation — an agent can self-discover the contract without reading this README.

### Integration patterns

**Blocking** — for environments where the calling shell can wait on a long-running command:

```sh
result=$(ronten review --base main --concerns concerns.json)
echo "$result" | jq -r '.decision'
```

**Background + polling** — for agent shells with hard command timeouts (e.g. Claude Code's
bash tool), where the review may take longer than the shell will wait. `--out` is only
written on a successful submission (exit 0/1); on abort (exit 2) or timeout (exit 3) no
file ever appears, so the loop must also watch the process itself rather than only the
file:

```sh
ronten review --base main --concerns concerns.json --out result.json --no-open &
RONTEN_PID=$!

while [ ! -f result.json ] && kill -0 "$RONTEN_PID" 2>/dev/null; do
  sleep 5
done
wait "$RONTEN_PID"
EXIT_CODE=$?
# result.json exists only when EXIT_CODE is 0 or 1; on abort/timeout there is
# no result file and the exit code is the only signal.
```

Either way, `ronten review` remains a single foreground-equivalent process for the duration
of the review — nothing is left running once a result exists.

## Examples

### Concerns JSON (input)

```jsonc
{
  "version": 1,
  "summary": "Introduce auth middleware and adjust route definitions",
  "concerns": [
    {
      "id": "auth-core",
      "title": "Auth middleware core",
      "description": "Validates JWT … (agent's intent/rationale, markdown allowed)",
      "risk": "high",
      "locations": [
        { "path": "src/middleware/auth.ts" },
        { "path": "src/routes/index.ts", "side": "new", "start": 10, "end": 42 }
      ]
    }
  ]
}
```

### Result JSON (output)

```jsonc
{
  "version": 1,
  "decision": "request-changes",
  "concerns": [
    {
      "id": "auth-core",
      "verdict": "request-changes",
      "comments": [
        {
          "path": "src/middleware/auth.ts",
          "side": "new",
          "line": 27,
          "body": "No log on the token-expiry branch"
        }
      ]
    },
    { "id": "_unmapped", "verdict": "approve", "comments": [] }
  ],
  "general_comments": ["Overall, …"],
  "warnings": ["location out of range: src/routes/index.ts:120-140"],
  "started_at": "…", "submitted_at": "…"
}
```

`decision` is derived from the per-concern verdicts: any `request-changes` verdict makes
the overall decision `request-changes`. Agents can feed `concerns[].comments` directly into
a fix loop. On abort or timeout, no result JSON is produced at all — the exit code (2 or 3)
is the only signal; the `abort` decision value is reserved and not currently emitted.

## Keyboard shortcuts

The UI is keyboard-first — a full review pass is possible without a mouse:

| Key | Action |
|---|---|
| `j` / `k` | select next / previous concern |
| `a` | verdict: approve |
| `x` | verdict: request changes |
| `c` | verdict: comment |
| `i` | focus the comment box |
| `Enter` | confirm submit |
| `Escape` | close the submit/abort confirmation or the inline comment editor |

## Security

- The server binds `127.0.0.1` only — never reachable off-machine.
- Every session gets a random per-session token embedded in the URL path
  (`/r/<token>`); all API routes require it, preventing same-machine snooping or forged
  submissions via port scanning.
- Submission is accepted once per process lifetime; a second `submit` call gets `409`.

**Trust boundary**: the token (together with the localhost bind and single-submit rule)
protects against port scanning and accidental access by other processes on the same
machine. It does not protect against the agent that launched ronten: the session URL is
printed to stderr, so that agent necessarily knows the token. The design assumes a
trusted-but-fallible agent — it keeps an honest agent from misrepresenting the diff, but
it cannot stop a malicious (or prompt-injected) agent from forging the human's verdict.

## Non-goals for v0.1

- GitHub/GitLab comment publishing or PR integration (future `ronten publish`).
- Review history persistence, multi-reviewer support, authentication.
- Editing the diff — fixing issues remains the agent's job.
- Native Windows (WSL is fine; targets are macOS/Linux).
- Virtual scrolling in the diff view (only the selected concern's hunks render, with large
  hunks collapsed by default).
- Agent self-reported metadata fields in concerns JSON (unknown fields are ignored today,
  leaving room to add them later).

## Development

### Repo layout

```
src/
  main.rs        — CLI parsing, dispatch, exit-code mapping
  model.rs       — serde types for concerns/result + schemars derives
  gitdiff.rs     — git invocation + unified diff parsing
  mapping.rs     — hunk × location intersection, _unmapped synthesis, warnings
  session.rs     — session state (diff + concerns + draft)
  server.rs      — axum routes, token check, static assets
  review.rs      — orchestration for `ronten review`
  demo.rs        — orchestration for `ronten demo` (embedded fixtures, no git)
  schema_cmd.rs  — `ronten schema`
  assets.rs      — embedded frontend static assets
frontend/        — Svelte app (npm project; build output embedded into the binary)
fixtures/        — demo diff + concerns used by `ronten demo`
tests/           — Rust integration tests (real git repos in tempdirs)
```

### Rust

```sh
cargo test
```

### Frontend dev loop

Run the backend without rebuilding the frontend on every change, then point Vite's dev
server at it:

```sh
cargo run -- demo --no-open
```

```sh
cd frontend
RONTEN_DEV_API=http://127.0.0.1:<port> npm run dev
```

(`<port>` is printed to stderr by the `demo` command above.) Vite proxies `/api` requests to
the running Rust process, so frontend edits hot-reload against a live session.

Other frontend commands:

```sh
cd frontend
npm run check   # svelte-check + tsc
npm run test    # vitest
npm run build   # production build into frontend/dist
```

### Skipping the frontend build

`cargo build`/`cargo test` normally rebuild the embedded frontend via `build.rs`, which
requires Node.js. To skip that (e.g. iterating on Rust-only changes) when `frontend/dist`
already exists from a previous build:

```sh
RONTEN_SKIP_FRONTEND_BUILD=1 cargo test
```

## License

MIT
