# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the
project is pre-1.0 and minor versions may contain breaking changes.

## [Unreleased]

### Added
- Result JSON output contract v2: a `review` block pinning every result to
  the reviewed commits (`base_oid` / `head_oid` / `merge_base_oid`),
  canonical SHA-256 digests of the diff and concerns input, a session id,
  the ronten version, and an explicit `"assurance": "advisory"` marker.
- Submit-time freshness check: if `HEAD` no longer matches the reviewed
  commit, submit is refused with `409 review stale`.
- `--dirty-policy error|warn|ignore` (default `error`, exit 17): a dirty
  worktree — including untracked files, the classic forgotten `git add` —
  refuses to start the review instead of silently reviewing less than the
  change.
- File mode / type visibility: per-side `FileType`
  (regular/executable/symlink/gitlink), always-visible mode/type
  transitions, submodule-pointer and Git LFS pointer notices, and explicit
  acknowledgements for gitlink pointer changes and mode changes.
- Per-line EOL metadata: LF→CRLF changes and missing final newlines are
  rendered visibly instead of as identical-looking lines.
- Structured warnings (`code` / `severity` / `message` / `path` /
  `concern_id`) end to end, preserved in the result JSON.
- Resource budgets (bounded-refuse): per-file and whole-review byte/line
  limits degrade to explicitly-acknowledged "too large" cards; more than
  2000 changed files refuses to start (exit 18); every git subprocess runs
  under a 60s deadline and is killed on overrun.
- Draft revisions: saves and submits carry a monotonically increasing
  revision; a stale tab gets `409 draft conflict` instead of silently
  overwriting (or submitting past) another tab's newer edits.
- Explicit 8 MiB request-body cap with a JSON 413; server-side limits are
  echoed to the UI, which enforces `maxlength` with a character counter.
- Release engineering: SHA-pinned actions, pinned toolchains, cargo
  deny/audit gates, packaged-crate install smoke test, and a tag-triggered
  draft-release workflow producing binaries, SHA256SUMS, an SPDX SBOM, and
  build-provenance attestations.
- `--out` no-clobber preflight: the target must not already exist, and is
  refused (exit 15) if it is a tracked file, falls inside the git
  directory, is the same file as `--concerns`, is a directory, or is a
  symlink. After the dirty gate, the path is reserved atomically
  (`O_CREAT|O_EXCL`, mode 0600) as an empty placeholder, best-effort
  removed again on abort, timeout, or error.
- Control characters (C0 including TAB/LF, C1, DEL, U+2028/U+2029) in
  worktree paths and git error text are escaped to visible `⟨U+XXXX⟩`
  tokens on stderr; the review UI reveals the same ranges in paths and
  titles (and all but TAB in diff line content).
- Result JSON output contract v3, so a result is auditable standalone
  without re-deriving anything from the live session: `files[]` — every
  file in the diff, with `content_kind` and, when its content wasn't
  rendered, an `omission_reason` (`binary`/`lfs_pointer`/`too_large`/
  `non_utf8`/`submodule`); `acknowledgements[]` — `file_id`, the
  server-computed ack reasons, and an RFC3339 `acknowledged_at` (submit
  time); `worktree` — the `--dirty-policy` in effect and whether the
  worktree was checked/clean at session start and re-checked at submit
  (fail-open: a git failure at the submit recheck is a warning, not a
  blocker); `build` — `ronten_version` now, with
  `source_commit`/`rust_version`/`target`/`profile`/`frontend_digest`
  wired but `null` until a future build.rs change populates them. A
  modified binary file now also emits a structured `BINARY_CONTENT`
  warning instead of passing through silently.
- New `ronten validate-concerns [FILE|-]` subcommand: validates a
  concerns JSON document (file, stdin via `-`/omitted, or piped) with the
  same structural parse and semantic checks `ronten review` runs at
  startup, without needing a git repository or opening a session. Prints
  `{"valid": true}` (exit 0) or `{"valid": false, "errors": [{"code",
  "message", "concern_id"?}]}` (exit 10, the same code `review` uses for
  invalid concerns).

### Changed
- File acknowledgement is now server-authoritative and keyed by a stable,
  index-independent `file_id` (SHA-256 of path/oid/mode) instead of an
  array index: `Draft.acknowledged_opaque: Vec<usize>` is now
  `Draft.acknowledgements: Vec<String>`. The session payload carries a
  computed `ack_required`/`ack_reasons` per file — the frontend only
  displays these, it no longer recomputes the policy itself (previously
  duplicated in `opaque.ts`'s `requiresAck`, which could drift). The ack
  policy also grew: a newly added or deleted symlink, a newly added
  executable, a regular file becoming a symlink, and an LFS pointer now
  require acknowledgement (previously only opaque content, a gitlink
  pointer move, and a both-sided mode change did); one-sided mode/type
  badges (`added symlink 120000`, `added executable 100755`, `deleted
  symlink 120000`) are always shown in the file header.
- Terminal state, outcome, and the editable draft are one atomic value: no
  submit/abort/timeout race can lose an outcome, double-report, or let a
  late autosave rewrite a frozen draft. Graceful shutdown has a hard
  deadline; a second Ctrl-C force-exits.
- Result JSON `version` is now `3`; `warnings` are structured objects. The
  concerns *input* contract remains v1.
- `ConcernsInput.version` and `ResultOutput.version` are now pinned via a
  real JSON Schema `const` (was an approximating `min == max` range), and
  `started_at`/`submitted_at`/`acknowledged_at` are marked `format:
  date-time`. `mapping::validate_concerns` now returns every applicable
  validation failure (not just the first), each with a stable
  machine-readable `code`; `review`'s startup check and the new
  `validate-concerns` subcommand share this one validator, so they can
  never report different results for the same input.
- Comment limits are now mutually consistent end to end: new review-wide
  totals (1000 comments, 1,500,000 comment characters) guarantee a draft
  satisfying every advertised per-field limit still fits the 8 MiB
  request body cap; `PUT /draft` now enforces these caps too (422 "draft
  exceeds limits"), not just submit. The browser measures draft byte size
  (warns at 90%, blocks at 100% of the advertised max) and now
  counts/truncates comment text by Unicode scalar instead of UTF-16 code
  units, matching how the server counts characters.
- The dirty-worktree gate's exemption is now category- and path-scoped:
  only the untracked concerns input file, matched by its exact
  repo-relative path (no symlink resolution), is exempt; tracked changes
  and dirty submodules are never exempt, and the `--out` target is no
  longer exempt either.
- On submit, the result is written to `--out` before stdout; a broken
  stdout pipe no longer aborts the process or skips the `--out` write.
- Review timeout now has its own terminal screen instead of being folded
  into the aborted screen; the session payload includes a UTC
  `deadline_at` when `--timeout` is set, and the review screen shows a
  live countdown to it.
- The review UI precomputes a `(file, hunk)` → concern hunk-owner index
  once per session load instead of re-walking every concern's hunks on
  each rendered hunk, and now force-collapses all of a selected concern's
  hunks when their combined rendered length exceeds 1,000 lines, on top
  of the existing per-hunk 200-line collapse — bounding initial DOM
  without virtualization.

### Fixed
- The opaque-content ack card (binary/non-UTF-8/too-large) now also shows
  any other applicable ack reasons (e.g. a mode change on the same file)
  instead of only the generic "acknowledge without reviewing" checkbox.
- Save, submit, and abort are serialized through a single mutation chain,
  and submit now flushes any in-flight or queued autosave before reading
  the revision to submit: previously submit only cancelled the pending
  debounce timer, so editing during an autosave could get a spurious
  `409 draft conflict` and lose the newer text on the forced reload.
- Save and submit requests carry a client-generated `mutation_id`; a
  repeat request with the same id replays the original result instead of
  re-applying the save or 409ing the submit, so a lost HTTP response can
  be retried without double-applying a save or a false conflict — a
  repeat submit with the same id returns the original outcome even if
  `HEAD` has since moved.
- Corrected the submit timeout hierarchy: the `HEAD` freshness check now
  uses its own 10s deadline, the server request timeout is 30s, and the
  browser fetch timeout is 40s (was 15s), so internal < server < client
  and the browser no longer gives up before the server could have
  finished. On an ambiguous submit/save failure (an aborted request or
  network error, not a clean HTTP rejection), the UI now queries the
  session once and either recovers to the submitted state, shows a
  retryable error, or shows a distinct "result unknown" state, instead
  of assuming failure.
- In-progress inline and general comment text is now held in the shared
  review state instead of component-local state, so it survives
  switching concerns or closing the editor and is covered by the
  unsaved-changes warning instead of silently disappearing; submitting
  with unsent comment text now prompts for confirmation.
- Git subprocesses now run in their own process group, and the whole
  group (not just the direct child) is killed on the 60s deadline, so a
  descendant holding a pipe open can no longer wedge the parent past the
  timeout. Subprocess stdout is capped per call (64 MiB for `diff-tree`,
  16 MiB for `status`, 8 MiB otherwise) and stderr is kept as a bounded
  8 KiB tail instead of being read unbounded.
- `diff-tree` and `git status` output is now parsed with entry-count caps
  checked during parsing, not after: a diff touching more than 2000 files
  is refused (exit 18) before every entry is materialized, and a worktree
  with more than 10,000 status entries is treated as dirty (never clean)
  without enumerating them all.
- Concern location intervals are normalized (sorted and merged) per
  `(concern, path, side)` before walking changed lines, and total
  resolved edges (1,000,000) and hunk references (100,000) are now
  hard-capped across a review, so a legal but pathological input (e.g.
  200 concerns × 200 locations on one file) is bounded or refused (exit
  18) instead of doing unbounded work.

## [0.1.0] - 2026-07

Initial release: concern-based review UI over `git diff <base>...HEAD`,
tamper-resistant diff reconstruction from blob objects, concern→line
mapping with an `_unmapped` bucket, opaque-content acknowledgements,
single-submit sessions, and machine-readable result JSON (v1).
