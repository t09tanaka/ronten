# PR1: Diff Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the two P0 diff-integrity holes — Git replacement refs hiding real HEAD changes, and non-UTF-8 blobs collapsing to identical lossy strings — plus make the raw diff-tree parser fail-closed.

**Architecture:** All hardening lives in `src/gitdiff.rs`. A single sanitized `base_git()` command builder (used by `git_cmd`, `repo_root`, `current_branch`) adds `--no-replace-objects` / `GIT_NO_REPLACE_OBJECTS=1` and strips repo-redirection env vars. `compute_diff` gains a `FileStatus::NonUtf8` path so byte-different non-UTF-8 blobs are never rendered as "no content changes". `parse_raw_z` returns `Result` and errors on malformed records instead of silently dropping them.

**Tech Stack:** Rust (std::process::Command, serde), Svelte 5 + TypeScript frontend, real-git fixture tests via `tempfile`.

## Global Constraints

- 全PRはmainベース・`gh pr create`経由（ローカルマージ禁止、`--amend`禁止）
- `frontend/dist` はコミット対象（frontend変更時は再ビルドしてdistもコミット）
- Push前に `/run-github-actions-locally` でローカルCI確認（fmt / clippy -D warnings / cargo test / frontend check+test+build）
- テスト・lint実行は sonnet subagent に委譲する（親コンテキストで直接実行しない）
- 出力契約（result JSON）は変更しない。`FileStatus` はsession payload（フロントエンド専用）なので追加は非破壊

---

### Task 1: Git command hardening (`--no-replace-objects` + env scrub)

**Files:**
- Modify: `src/gitdiff.rs:424-457` (`repo_root`, `current_branch`), `src/gitdiff.rs:501-513` (`git_cmd`)
- Test: `src/gitdiff.rs` `mod git_tests`

**Interfaces:**
- Produces: `fn base_git() -> std::process::Command`（サニタイズ済みベースコマンド）。`git_cmd(root)` は `base_git()` + `-C <root>`。既存の公開シグネチャ（`repo_root`, `current_branch`, `compute_diff`）は不変。

- [ ] **Step 1: Write the failing test**

`mod git_tests` に追加（`git_stdout` ヘルパーも新設）:

```rust
    /// Runs git in `dir` and returns trimmed stdout (asserting success).
    fn git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn replace_refs_cannot_hide_changes() {
        // An agent can `git replace` the real HEAD with a fake commit whose
        // tree equals the base tree; every git command then silently reads
        // the fake commit and the diff comes back empty. --no-replace-objects
        // must defeat this.
        let td = base_repo();
        let d = td.path();
        std::fs::write(d.join("a.txt"), "one\nEVIL\nthree\n").unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "evil change"]);
        let head = git_stdout(d, &["rev-parse", "HEAD"]);
        let base_tree = git_stdout(d, &["rev-parse", "main^{tree}"]);
        let fake = git_stdout(d, &["commit-tree", &base_tree, "-p", "main", "-m", "innocent"]);
        git(d, &["replace", &head, &fake]);

        let out = compute_diff(d, "main").unwrap();
        let a = find(&out.files, "a.txt");
        assert_eq!(a.status, FileStatus::Modified);
        assert!(hunk_contents(a).contains(&"EVIL"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test replace_refs_cannot_hide_changes`
Expected: FAIL（`compute_diff` が空 or `find` が panic — replacement ref が効いて diff が消えるため）

- [ ] **Step 3: Implement `base_git()` and route all callers through it**

`git_cmd` を置き換え、`repo_root` / `current_branch` も同じベースを使う:

```rust
/// Environment variables that can redirect which repository/objects git
/// reads, or alter diff output. The reviewed agent shares this process
/// environment, so all of them are stripped before running git.
const SCRUBBED_GIT_ENV: &[&str] = &[
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Base `git` invocation: replacement refs disabled (an in-repo agent can
/// `git replace` HEAD with an innocent-looking commit), repo-redirection
/// env stripped, and `LC_ALL=C` for stable message parsing.
fn base_git() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--no-replace-objects")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("LC_ALL", "C");
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

fn git_cmd(root: &std::path::Path) -> std::process::Command {
    let mut cmd = base_git();
    cmd.arg("-C").arg(root);
    cmd
}
```

`repo_root()` は `base_git().args(["rev-parse", "--show-toplevel"])` に、`current_branch()` は `git_cmd(root).args(["rev-parse", "--abbrev-ref", "HEAD"])` に変更（`current_dir`+生Command をやめる）。既存の doc comment（`git_cmd` の「cheap defense in depth」段落）は新実装に合わせて書き直す。

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --lib` （gitdiff の全テスト）
Expected: PASS（既存テスト含む全緑）

- [ ] **Step 5: Commit**

```bash
git add src/gitdiff.rs
git commit -m "fix: disable git replacement refs and scrub repo-redirection env"
```

---

### Task 2: `FileStatus::NonUtf8` — 非UTF-8 blobをlossy diffしない

**Files:**
- Modify: `src/gitdiff.rs:3-13` (`FileStatus`), `src/gitdiff.rs:959-969` (`compute_diff` の `Plan::Content` 分岐)
- Modify: `frontend/src/lib/types.ts:26`, `frontend/src/lib/DiffView.svelte:30-45`
- Test: `src/gitdiff.rs` `mod git_tests`

**Interfaces:**
- Produces: `FileStatus::NonUtf8`（serde名 `"non-utf8"`）。hunkなし・mappingはBinary同様「whole-file locationのみclaim可」。session payload の `files[].status` に新値が増える（フロントは同一コミットで対応）。

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn non_utf8_blobs_are_not_lossy_diffed() {
        // 0x80 -> 0x81: neither contains NUL, both are invalid UTF-8, and
        // both lossy-decode to "\u{FFFD}\n" — a lossy text diff would claim
        // "no content changes" for a real byte-level change.
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        git(d, &["init", "-b", "main"]);
        git(d, &["config", "user.email", "t@example.com"]);
        git(d, &["config", "user.name", "t"]);
        std::fs::write(d.join("data.txt"), [0x80, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "base"]);
        git(d, &["checkout", "-b", "feature"]);
        std::fs::write(d.join("data.txt"), [0x81, b'\n']).unwrap();
        git(d, &["add", "."]);
        git(d, &["commit", "-m", "change"]);

        let out = compute_diff(d, "main").unwrap();
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0].status, FileStatus::NonUtf8);
        assert!(out.files[0].hunks.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("data.txt"),
            "warning should name the file: {}",
            out.warnings[0]
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test non_utf8_blobs_are_not_lossy_diffed`
Expected: FAIL（現状は `Modified` + hunks空で通ってしまう→ `NonUtf8` variant が無いのでコンパイルエラー。variant追加後は assert 失敗）

- [ ] **Step 3: Implement**

`FileStatus` に追加:

```rust
    #[serde(rename = "non-utf8")]
    NonUtf8,
```

`compute_diff` の `Plan::Content` 分岐を差し替え（`warnings` はこの時点でまだ可変）:

```rust
            Plan::Content => {
                let old = blob_of(&contents, &entry.old_oid);
                let new = blob_of(&contents, &entry.new_oid);
                if is_binary(old) || is_binary(new) {
                    (FileStatus::Binary, Vec::new())
                } else {
                    match (std::str::from_utf8(old), std::str::from_utf8(new)) {
                        (Ok(old_text), Ok(new_text)) => {
                            (entry_status(entry), text_hunks(old_text, new_text))
                        }
                        _ => {
                            // Different byte contents can lossy-decode to the
                            // same string; never diff lossily.
                            warnings.push(format!(
                                "file content is not valid UTF-8, not rendered: {}",
                                display_path(entry)
                            ));
                            (FileStatus::NonUtf8, Vec::new())
                        }
                    }
                }
            }
```

（注意: 現在 `warnings` はplansループ後に使われるだけなので、このループでの `warnings.push` は借用エラーにならない。`files` 構築ループ内で使うため `let mut warnings` のまま）

- [ ] **Step 4: Update frontend types and status card**

`frontend/src/lib/types.ts:26`:

```ts
export type FileStatus =
  | 'modified'
  | 'added'
  | 'deleted'
  | 'renamed'
  | 'binary'
  | 'non-utf8'
  | 'too-large'
```

`frontend/src/lib/DiffView.svelte` の `statusCardText` に追加:

```ts
      case 'non-utf8':
        return 'File content is not valid UTF-8 (not rendered)'
```

- [ ] **Step 5: Run all tests + frontend check, rebuild dist**

Run: `cargo test` および `cd frontend && npm run check && npm run test && npm run build`
Expected: 全PASS。`git status` で `frontend/dist` の差分有無を確認（ビルドハッシュが変われば dist もコミット対象）

- [ ] **Step 6: Commit**

```bash
git add src/gitdiff.rs frontend/src/lib/types.ts frontend/src/lib/DiffView.svelte frontend/dist
git commit -m "fix: report non-UTF-8 blobs as opaque instead of lossy text diff"
```

---

### Task 3: `parse_raw_z` fail-closed

**Files:**
- Modify: `src/gitdiff.rs:544-585` (`parse_raw_z`), `src/gitdiff.rs:878` (呼び出し側)
- Test: `src/gitdiff.rs`（新設 `mod raw_tests` または既存 `tests` mod 内）

**Interfaces:**
- Produces: `fn parse_raw_z(bytes: &[u8]) -> Result<Vec<RawEntry>, GitError>`。不正レコードは `GitError::GitFailed` で全体エラー（部分成功しない）。

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn parse_raw_z_valid_record() {
        let raw = b":100644 100755 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0a.txt\0";
        let entries = parse_raw_z(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].status, 'M');
    }

    #[test]
    fn parse_raw_z_rejects_garbage_meta() {
        // A token that isn't `:`-prefixed is not a diff-tree raw record;
        // silently skipping it would desynchronize the path tokens.
        assert!(parse_raw_z(b"garbage\0a.txt\0").is_err());
    }

    #[test]
    fn parse_raw_z_rejects_truncated_record() {
        // Meta token with no following path token.
        assert!(parse_raw_z(b":100644 100644 111 222 M\0").is_err());
        // Rename record missing its second path.
        assert!(parse_raw_z(
            b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R90\0old.txt\0"
        )
        .is_err());
        // Meta with fewer than 5 fields.
        assert!(parse_raw_z(b":100644 100644 M\0a.txt\0").is_err());
    }
```

（テストは `parse_raw_z` と同モジュールから見える位置に置く。`RawEntry` は private なので `#[cfg(test)] mod raw_tests { use super::*; ... }` を `parse_raw_z` 定義の近くに新設する）

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test parse_raw_z`
Expected: コンパイルエラー（戻り値が `Vec` のため `.unwrap()`/`.is_err()` が無効）

- [ ] **Step 3: Implement fail-closed parser**

```rust
/// Parses `git diff-tree -r -z --raw` output:
/// `:<oldmode> <newmode> <oldoid> <newoid> <status>\0<path>\0[<path2>\0]`.
/// Paths are NUL-delimited so they arrive verbatim (no quoting/escaping).
/// Any structurally malformed record is a hard error: this parser feeds the
/// review gate, so partial success is worse than failing the whole diff.
fn parse_raw_z(bytes: &[u8]) -> Result<Vec<RawEntry>, GitError> {
    let malformed = |detail: &str| GitError::GitFailed(format!("unexpected diff-tree output: {detail}"));
    let mut tokens = bytes.split(|&b| b == 0).peekable();
    let mut entries = Vec::new();
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            // The trailing NUL leaves one empty token; anything after it
            // would be malformed and is caught on the next iteration.
            continue;
        }
        let meta = String::from_utf8_lossy(token).to_string();
        let Some(meta) = meta.strip_prefix(':') else {
            return Err(malformed(&format!("record does not start with ':': {meta:?}")));
        };
        let parts: Vec<&str> = meta.split(' ').collect();
        if parts.len() < 5 {
            return Err(malformed(&format!("record has {} fields (expected 5): {meta:?}", parts.len())));
        }
        let status = parts[4].chars().next().ok_or_else(|| malformed("empty status field"))?;
        let path_token = tokens
            .next()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| malformed(&format!("record {meta:?} has no path")))?;
        let path = String::from_utf8_lossy(path_token).to_string();
        let path2 = if matches!(status, 'R' | 'C') {
            let token = tokens
                .next()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| malformed(&format!("rename/copy record {meta:?} has no second path")))?;
            Some(String::from_utf8_lossy(token).to_string())
        } else {
            None
        };
        entries.push(RawEntry {
            old_mode: parts[0].to_string(),
            new_mode: parts[1].to_string(),
            old_oid: parts[2].to_string(),
            new_oid: parts[3].to_string(),
            status,
            path,
            path2,
        });
    }
    Ok(entries)
}
```

呼び出し側（`compute_diff`）: `let entries = parse_raw_z(&out.stdout)?;`

（パスの `from_utf8_lossy` はこのPRでは維持 — 非UTF-8パスの扱い（fail-closed化 or file_id導入）はPR2のスコープ）

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: 全PASS

- [ ] **Step 5: Commit**

```bash
git add src/gitdiff.rs
git commit -m "fix: fail closed on malformed diff-tree raw records"
```

---

### Task 4: ローカルCI → push → PR作成

- [ ] **Step 1:** `/run-github-actions-locally` 相当のローカルチェック（fmt / clippy / cargo test / frontend check+test+build）を subagent で実行し全緑を確認
  - 注意: リモートCIは既存のdist鮮度問題（PR4で対応）でPackageステップが赤くなる可能性がある。ローカル検証を green 判定の根拠とする
- [ ] **Step 2:** `git push -u origin feature/diff-integrity` → `gh pr create --base main`（PR本文に P0-1/P0-2 対応と攻撃シナリオの説明、レビュー由来である旨を記載）
- [ ] **Step 3:** ユーザーにマージを依頼し、マージ後にPR2の計画へ進む

---

## 後続PRロードマップ（各PR着手時に個別プランを作成）

- **PR2 (file model):** `change_kind`/`content_kind` の分離、old/new mode・OID・size を `FileDiff` に追加、binary/non-utf8/too-large の明示ack UI、非UTF-8パスの fail-closed 化（wire format変更なしのシンプル案を採用予定）、Bidi制御文字・不可視Unicodeの可視化＋`unicode-bidi: isolate`
- **PR3 (mapping contract):** v1入力に `deny_unknown_fields`、changed-line単位のcoverage、`_unmapped` を未claim add/remove行から生成、`request-changes` 理由必須のサーバー側強制、`Decision::Abort` の整理、入力サイズ上限＋pathインデックス化
- **PR4 (release/運用):** dist鮮度問題の根本解決（Linux環境で差分原因を特定）＋package job分離・required化＋`fail-fast: false`、atomic `--out`＋書き込み失敗の専用exit code、server task異常終了の伝播、security/cache headers、dirty worktree警告、`rust-version = "1.82"`
