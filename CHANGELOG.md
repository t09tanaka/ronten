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

### Changed
- Terminal state, outcome, and the editable draft are one atomic value: no
  submit/abort/timeout race can lose an outcome, double-report, or let a
  late autosave rewrite a frozen draft. Graceful shutdown has a hard
  deadline; a second Ctrl-C force-exits.
- Result JSON `version` is now `2`; `warnings` are structured objects. The
  concerns *input* contract remains v1.

## [0.1.0] - 2026-07

Initial release: concern-based review UI over `git diff <base>...HEAD`,
tamper-resistant diff reconstruction from blob objects, concern→line
mapping with an `_unmapped` bucket, opaque-content acknowledgements,
single-submit sessions, and machine-readable result JSON (v1).
