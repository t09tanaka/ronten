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
| `--out <path>` | none | Also write result JSON to a file (in addition to stdout); the target must not already exist |
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
proceeds; `--dirty-policy ignore` skips the check. Only the concerns file is exempt (when
it shows up untracked at exactly its own path — ronten itself expects it in the worktree);
tracked changes and dirty submodules are never exempt, and the `--out` destination is not
exempt either, since it is only reserved on disk after this gate runs. Under the default
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
| 15 | `--out` rejected or failed: the target already exists, is tracked by git, is inside `.git`, is the same file as `--concerns`, is a directory/symlink, or the final write itself failed |
| 16 | the server task terminated unexpectedly (e.g. a panic) before an outcome was reached |
| 17 | worktree not clean under `--dirty-policy error` (the default); commit/stash first or pass `--dirty-policy warn` |
| 18 | the diff exceeds a hard resource budget (e.g. more than 2000 changed files); review it in smaller pieces |

This table describes `ronten review` only. `ronten validate-concerns` has its own, narrower
exit codes (0 valid / 10 invalid) — see the [`ronten validate-concerns`](#ronten-validate-concerns)
section below; its exit 0 means "structurally and semantically valid concerns JSON", not "review
approved".

### `ronten schema`

```sh
ronten schema             # both schemas
ronten schema --input     # concerns JSON Schema only
ronten schema --output    # result JSON Schema only
```

Prints the JSON Schemas for the concerns input and result output to stdout. The schemas are
generated from the same serde types the binary uses at runtime (via `schemars`), so field
names, types, and simple constraints (lengths, ranges, the pinned `version` `const`) can
never drift from the implementation.

This is a **structural** schema only. Semantic constraints that depend on the whole document
— no duplicate concern ids, no blank-after-trim titles, `start <= end`, the reserved
`_unmapped` id — are enforced at runtime and are not (and cannot fully be) expressed in JSON
Schema. To check those, run `ronten validate-concerns` instead of hand-validating against
the schema.

### `ronten validate-concerns`

```sh
ronten validate-concerns concerns.json   # validate a file
ronten validate-concerns -               # validate from stdin
cat concerns.json | ronten validate-concerns
```

Parses a concerns JSON document and runs the same semantic validation `ronten review` runs at
startup — without needing a git repository or opening a session. Always prints one JSON object
to stdout:

- Valid: `{"valid": true}`, exit 0.
- Invalid: `{"valid": false, "errors": [{"code": "...", "message": "...", "concern_id": "..."}]}`,
  exit 10 (the same code `ronten review` exits with for invalid concerns). `concern_id` is
  present only for failures scoped to a specific concern; `code` is a stable, machine-readable
  identifier (e.g. `DUPLICATE_CONCERN_ID`, `START_AFTER_END`, `RESERVED_CONCERN_ID`).

### Integration patterns

**Blocking** — for environments where the calling shell can wait on a long-running command:

```sh
result=$(ronten review --base main --concerns concerns.json)
echo "$result" | jq -r '.decision'
```

**Background + polling** — for agent shells with hard command timeouts (e.g. Claude Code's
bash tool), where the review may take longer than the shell will wait. **The `--out` path
must not exist yet**: `ronten review` refuses to start (exit 15) if the target already
exists (a stale result left over from a previous run), is tracked by git, sits inside
`.git`, is the same file as `--concerns`, or is a directory/symlink — move or delete a
leftover `result.json` before re-running. Because the target is reserved as an empty
placeholder as soon as the session starts (well before a decision is made — see below),
`[ -f result.json ]` becomes true almost immediately and is *not* a completion signal;
poll the process instead and only trust the file once it has exited:

```sh
ronten review --base main --concerns concerns.json --out result.json --no-open &
RONTEN_PID=$!

while kill -0 "$RONTEN_PID" 2>/dev/null; do
  sleep 5
done
wait "$RONTEN_PID"
EXIT_CODE=$?
# result.json holds the real result only when EXIT_CODE is 0 or 1. On
# abort/timeout (2/3) it is absent again. Exit 15 splits in two: if --out
# was refused before the session ever started (e.g. the target already
# existed from a previous run), that pre-existing file is left present and
# UNTOUCHED — it never held this run's result and must not be assumed
# empty/absent or auto-deleted. If the reservation succeeded but the final
# write itself failed after a decision was reached, the placeholder is
# removed, so the file is genuinely absent again. Either way, the exit code
# is the only reliable signal of the outcome; treat any pre-existing
# result.json on a 15 as needing manual triage, not cleanup.
```

Either way, `ronten review` remains a single foreground-equivalent process for the duration
of the review — nothing is left running once the process exits.

`--out` is reserved and written atomically, closing the two races an agent-facing poller
would otherwise be exposed to: a stale file silently getting overwritten, and a poller
observing a partially-written file. As soon as the session starts, ronten atomically
creates an empty placeholder at the target path (`O_CREAT|O_EXCL`, refusing to clobber
anything already there); on submission the result is written to a same-directory temp
file, flushed, then renamed over that placeholder, so the file is always either absent,
an empty reservation, or complete — never partial. On any non-submitted outcome (abort,
timeout, ctrl-c, or an error) the placeholder is removed again, so a subsequent run sees a
clean slate. `--out` is written before stdout, so its failure is confirmed (or fails loudly
with code 15) before the process risks losing the result to a stdout error. If the final
write itself fails (e.g. the parent directory disappeared mid-run), the process still exits
with the dedicated code 15; stdout is attempted regardless of that outcome, so the review
outcome itself is not lost, only the file copy.

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
  "version": 3,
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
  "files": [
    {
      "file_id": "3f9a2b7c1d4e5f60",
      "old_path": "src/middleware/auth.ts",
      "new_path": "src/middleware/auth.ts",
      "old_mode": "100644",
      "new_mode": "100644",
      "file_type": "regular",
      "old_oid": "1a2b3c…",
      "new_oid": "4d5e6f…",
      "content_kind": "text",
      "rendered": true
    },
    {
      "file_id": "8b1c4d2e9f0a3b7c",
      "old_path": null,
      "new_path": "assets/logo.png",
      "old_mode": null,
      "new_mode": "100644",
      "file_type": "regular",
      "old_oid": null,
      "new_oid": "7e8f9a…",
      "content_kind": "binary",
      "rendered": false,
      "omission_reason": "binary"
    }
  ],
  "acknowledgements": [
    {
      "file_id": "8b1c4d2e9f0a3b7c",
      "reasons": ["opaque-content"],
      "acknowledged_at": "2026-07-22T04:32:10Z"
    }
  ],
  "worktree": {
    "policy": "error",
    "checked_at_start": true,
    "clean_at_start": true,
    "checked_at_submit": true,
    "clean_at_submit": true,
    "excluded_paths": ["concerns.json"]
  },
  "build": {
    "ronten_version": "0.1.0",
    "source_commit": null,
    "source_dirty": null,
    "rust_version": null,
    "target": null,
    "profile": null,
    "frontend_digest": null
  },
  "started_at": "…", "submitted_at": "…"
}
```

`files` lists every file in the reviewed diff, including ones whose content was not shown
(`rendered: false` with an `omission_reason` of `binary`, `lfs_pointer`, `too_large`,
`non_utf8`, or `submodule`) — so a standalone reader of the JSON can reconstruct what was and
wasn't shown to the reviewer without re-running the diff. `acknowledgements` lists every file
the reviewer explicitly acknowledged before submit, keyed by the same `file_id` as `files`,
with the server-computed reasons acknowledgement was required and the submit-time timestamp.
`worktree` records the `--dirty-policy` in effect and whether the worktree was checked/clean
at session start and re-checked at submit (`checked_at_*` is `false` under `--dirty-policy
ignore`, or if the query itself failed). `build` identifies the binary that produced the
result; `source_commit`/`source_dirty`/`rust_version`/`target`/`profile`/`frontend_digest` are
`null` unless the build was compiled with the corresponding `build.rs`-injected env vars.

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
- Submission is accepted once per process lifetime; a second `submit` call with a
  different mutation id gets `409` (a repeat call with the *same* mutation id — a
  retry of a lost response — replays the original outcome instead).

**Trust boundary**: the token (together with the localhost bind and single-submit rule)
protects against port scanning and accidental access by other processes on the same
machine. It does not protect against the agent that launched ronten: the session URL is
printed to stderr, so that agent necessarily knows the token. The design assumes a
trusted-but-fallible agent — it keeps an honest agent from misrepresenting the diff, but
it cannot stop a malicious (or prompt-injected) agent from forging the human's verdict.

## Resource budgets

The diff pipeline is bounded-refuse, never unbounded-process, and nothing is silently
truncated:

- Per file: blobs over 1 MiB, more than 50,000 lines on either side, or any single line
  over 64 KiB degrade the file to an explicitly-acknowledged "too large" card with a
  structured warning (`FILE_TOO_LARGE` / `FILE_TOO_MANY_LINES` / `LINE_TOO_LONG`).
- Per review: 50 MiB of total blob content and 200,000 total rendered diff lines; files
  past either budget degrade the same way (`DIFF_TOO_LARGE`), which also bounds the session
  JSON. More than 2000 changed files refuses to start (exit 18) — that cannot be
  meaningfully reviewed in one sitting.
- The frontend does not render all 200,000 lines' worth of DOM at once — it bounds initial
  mount with collapsing, not virtualization: any single hunk over 200 lines starts
  collapsed, and if a selected concern's hunks collectively total over 1,000 rendered
  lines, every one of that concern's hunks starts collapsed too (each still expandable on
  demand). A concern's shared-hunk lookup (which other concerns also claim a given hunk)
  is precomputed once per session load rather than rescanned per rendered hunk.
- Every git subprocess runs in its own process group under a hard 60-second deadline; on
  overrun the whole group is killed (and reaped), not just the direct child, so a
  descendant holding a pipe open can no longer stall the review past the deadline.
  Subprocess stdout is also capped per call (64 MiB for diff-tree, 16 MiB for status, 8 MiB
  otherwise) and stderr is kept as a bounded 8 KiB tail, so a runaway git refuses rather
  than being read into memory unbounded.

## Non-goals for v0.1

- GitHub/GitLab comment publishing or PR integration (future `ronten publish`).
- Review history persistence, multi-reviewer support, authentication.
- Editing the diff — fixing issues remains the agent's job.
- Native Windows (WSL is fine; targets are macOS/Linux).
- Virtual scrolling in the diff view. Instead, hard resource budgets keep oversized content
  from ever reaching the browser (see "Resource budgets" below); only the selected concern's
  hunks render, with large hunks collapsed by default.
- Agent self-reported metadata fields in concerns JSON — v1 rejects unknown fields outright
  (exit 10, via `deny_unknown_fields`) rather than ignoring them; such fields would be added
  in a future version 2 of the contract, never by loosening v1.

## Releases and supply chain

CI pins every GitHub Action to a full commit SHA, pins the Rust toolchain
(`rust-toolchain.toml`) and Node patch version, and gates on `cargo deny`
(advisories / license allowlist / source provenance, see `deny.toml`) and
`cargo audit`. The `package` job installs the actual packaged `.crate` and
runs the binary, so "`cargo install ronten` works without Node.js" is enforced,
not assumed.

Tagging `vX.Y.Z` (which must equal the `Cargo.toml` version) first re-runs the
entire CI suite (fmt/clippy/test, MSRV, `cargo deny`/`cargo audit`, package,
frontend) at the exact tagged commit, and refuses to release unless that
commit is reachable from `origin/main` — a tag on an unreviewed or arbitrary
commit cannot produce binaries. Only once that passes are the platform
binaries built and attached to a **draft** GitHub release together with a
`SHA256SUMS` file (which also covers the SBOM below), an SPDX SBOM, and a
build-provenance attestation over both. Before publishing the draft, verify
an artifact end to end:

```sh
sha256sum --check SHA256SUMS
gh attestation verify ronten-<version>-<target>.tar.gz --repo t09tanaka/ronten
```

### Supported release platforms

| OS | Arch | Target triple | Notes |
| --- | --- | --- | --- |
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | Dynamically linked against glibc; built on Ubuntu 22.04, so the minimum glibc is that image's (2.35). |
| Linux | x86_64 | `x86_64-unknown-linux-musl` | Statically linked (musl libc) — no runtime glibc dependency at all; use this on distros with an older or different libc. |
| macOS | arm64 (Apple Silicon) | `aarch64-apple-darwin` | macOS 11+ (Big Sur). |
| macOS | x86_64 (Intel) | `x86_64-apple-darwin` | macOS 11+ (Big Sur). |

**macOS binaries are not signed or notarized.** Gatekeeper will warn ("cannot
be opened because the developer cannot be verified") on first launch; the
usual workaround is `xattr -d com.apple.quarantine <path-to-ronten>` or
right-click → Open. Signing/notarization needs an Apple Developer account and
its associated secrets, which this project does not have — it is explicitly
out of scope, not an oversight. Building from source (`cargo install
--locked ronten` or `cargo build --release`) sidesteps this entirely, since
the binary is compiled locally rather than downloaded.

Native Windows binaries are not built or supported (see "Non-goals" below);
use WSL.

Publishing to crates.io remains a deliberate manual `cargo publish`. See
[SECURITY.md](SECURITY.md) for the vulnerability-reporting process and threat
model, and [CHANGELOG.md](CHANGELOG.md) for release history.

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
  validate_cmd.rs — `ronten validate-concerns`
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

MIT.

The binary statically links its Rust dependencies and embeds a compiled frontend bundle,
so every release tarball (and the published crate) ships `THIRD_PARTY_NOTICES.md` with the
licenses and copyright notices of everything it carries. That file is generated by
`scripts/gen-third-party.sh` from `Cargo.lock` and `frontend/package-lock.json`; CI
regenerates it on every PR and fails if the committed copy is stale, so a dependency bump
cannot quietly leave the notices behind.
