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
is displayed. A concern claims individual changed lines, not whole hunks by range overlap;
any changed line that ends up claimed by no concern the agent proposed is never silently
dropped — it (and any hunk still containing such a line) is placed into an auto-generated,
warning-styled `_unmapped` concern that still requires a verdict from the human before
submission. The agent then reads the result JSON — including line-anchored comments —
straight into its fix loop.

ronten is daemonless: one review session is one process. It starts an HTTP server bound to
`127.0.0.1`, serves the review UI, waits for a submission (or abort, or timeout), prints the
result JSON to stdout, and exits. No background service, no state files, no shared ledger —
parallel worktrees can run simultaneous sessions on their own ports without conflict.

## Install

```sh
cargo install --locked ronten
```

`--locked` installs exactly the dependency versions pinned in the shipped `Cargo.lock`
rather than letting cargo re-resolve to newer semver-compatible versions at install time.

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
| `--dirty-policy <p>` | `error` | What to do when the worktree is not clean: `error` (exit 17), `warn` (print and proceed), `ignore` |

**Output separation is strict**: human-facing logs (e.g.
`Review session: http://127.0.0.1:PORT/r/TOKEN`) go to stderr. Machine-readable data — the
result JSON — goes to stdout only, and only stdout. An agent can always do
`result=$(ronten review ...)` and safely `jq` the output.

**The diff is `<base>...HEAD` only** (merge-base semantics): it covers committed state, not
the working tree. If the agent forgot to commit some of its work before starting the review,
those changes are reviewed nowhere — most dangerously a brand-new file it never `git add`ed,
which makes the review look complete while the new file is invisible to both the diff and
the reviewer. To catch this, `ronten review` runs a structured
`git status --porcelain=v2 -z --untracked-files=all` check right after computing the diff
and classifies the results: uncommitted changes to tracked files, untracked files, and
submodules whose worktree is dirty inside.

By default (`--dirty-policy error`) any of these refuses to start the review with exit
code 17 and a per-file listing on stderr. `--dirty-policy warn` prints the same listing and
proceeds; `--dirty-policy ignore` skips the check. The concerns file and the `--out`
destination are exempt — ronten itself expects them in the worktree. Under the default
`error` policy a failing `git status` also refuses to start (exit 14): an unverifiable
worktree must not silently pass the gate.

**Changes the diff body under-communicates require explicit acknowledgement** before
submit: content that isn't rendered (binary / non-UTF-8 / too-large files), submodule
pointer changes (only the commit pointer is shown — the submodule's own diff is not),
and file mode/type changes (the executable bit appearing, a file becoming a symlink).
Each is also surfaced as a structured warning, file headers always show `mode`/type
transitions, and Git LFS pointers are flagged (the pointer is shown, not the real data).
Line endings are preserved per line, so an LF→CRLF change or a removed final newline is
visible instead of rendering as two identical-looking lines.

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
| 15 | `--out` write failed (the review outcome still printed to stdout; only the file write failed) |
| 16 | the server task terminated unexpectedly (e.g. a panic) before an outcome was reached |
| 17 | worktree not clean under `--dirty-policy error` (the default); commit/stash first or pass `--dirty-policy warn` |

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

`--out` is written atomically: the result is written to a same-directory temp file, flushed,
then renamed into place. A poller watching for `result.json` to appear can never observe a
partially-written file — it either isn't there yet or is complete. If the write itself fails
(e.g. the parent directory doesn't exist), the process exits with the dedicated code 15
rather than the approve/request-changes code; the result JSON has still been printed to
stdout by that point, so the review outcome itself is not lost, only the file copy.

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

A location's `path` selects a file: an explicit `side: "old"` matches against the file's old
path (required for a deleted file, which has no new path); `side: "new"` or an omitted `side`
matches against the new path. Within the matched file, a location claims the individual
`Add`/`Remove` lines whose own line number (`new_no` for `Add`, `old_no` for `Remove`) falls
in `[start, end]` (defaulting to the whole file when omitted) — never a hunk's full range, and
never a context line. An omitted `side` claims on *both* numbering schemes at once (`Add` by
`new_no`, `Remove` by `old_no`) within whichever file the path resolved to.

### Result JSON (output)

```jsonc
{
  "version": 2,
  "review": {
    "session_id": "9f2c…",
    "ronten_version": "0.1.0",
    "base_ref": "main",
    "base_oid": "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
    "head_oid": "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3",
    "merge_base_oid": "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
    "diff_sha256": "…64 hex chars…",
    "concerns_sha256": "…64 hex chars…",
    "assurance": "advisory"
  },
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
  "warnings": [
    {
      "code": "LOCATION_MATCHED_NOTHING",
      "severity": "warning",
      "message": "location matched no changed lines: src/routes/index.ts:120-140",
      "path": "src/routes/index.ts",
      "concern_id": "auth-core"
    }
  ],
  "started_at": "…", "submitted_at": "…"
}
```

`decision` is one of exactly two values, `approve` or `request-changes` — there is no `abort`
decision — derived from the per-concern verdicts: any `request-changes` verdict makes the
overall decision `request-changes`. Agents can feed `concerns[].comments` directly into a fix
loop. On abort or timeout, no result JSON is produced at all; the exit code (2 or 3) is the
only signal.

The `review` block pins the result to exactly what was reviewed. `base_oid`, `head_oid`, and
`merge_base_oid` are the commits resolved when the session started (`null` only for
`ronten demo`, which has no repository); `diff_sha256` and `concerns_sha256` are SHA-256
digests of the canonical serialization of the rendered diff and the full concerns input. A
consumer acting on a result must compare `review.head_oid` against the commit it is about to
act on — a result for one commit never applies to another. ronten enforces the same rule at
its end: submit re-resolves `HEAD`, and if it no longer matches the reviewed commit (an extra
commit landed, a branch switch happened), the submit is refused with `409 review stale` and
the UI asks the reviewer to start a new session.

`assurance` is always `"advisory"`: the process that launches `ronten` can read the session
URL from stderr, so ronten cannot prove the submit came from a human rather than from the
launching agent itself. Treat the result as a well-structured record of a review session, not
as a cryptographic approval gate, and do not wire it directly into security-enforcing CI
rules.

## Keyboard shortcuts

The UI is keyboard-first — a full review pass is possible without a mouse:

| Key | Action |
|---|---|
| `j` / `k` | select next / previous concern |
| `a` | verdict: approve |
| `x` | verdict: request changes |
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
- Agent self-reported metadata fields in concerns JSON — v1 rejects unknown fields outright
  (exit 10, via `deny_unknown_fields`) rather than ignoring them; such fields would be added
  in a future version 2 of the contract, never by loosening v1.

## Development

### Repo layout

```
src/
  main.rs        — CLI parsing, dispatch, exit-code mapping
  model.rs       — serde types for concerns/result + schemars derives
  gitdiff.rs     — git invocation + unified diff parsing
  mapping.rs     — changed-line claiming, _unmapped synthesis, warnings
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

### Packaging: `frontend/dist` in the published crate

`frontend/dist/` is gitignored (it's a Vite build artifact, not source), but it is
deliberately included in the published crate via `Cargo.toml`'s `include`, so that
`cargo install ronten` needs no Node.js at all (see [Install](#install)). Because the
directory is gitignored, `cargo package`'s normal "working tree must be clean" check can
never pass for it, so CI's `package` job runs `cargo package --allow-dirty` and then
verifies the crate's actual contents directly — asserting `frontend/dist/index.html` and
hashed `assets/*.js`/`*.css` files are present in the package listing, and that
`node_modules` did not leak in — rather than relying on the (defeated) dirty-tree gate.

## License

MIT
