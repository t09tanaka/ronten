# PR4: Release & Ops Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 外部レビュー P0-3（CI red）と P2（運用品質）への対応 — `cargo package` を green にする CI 再構成、atomic `--out`、server task 異常終了の伝播、security/cache headers、dirty worktree 警告、MSRV 宣言、README 同期。

**Architecture:** `frontend/dist` は「gitignore された生成物を crate に同梱する」現行設計を維持し、package ジョブは dist をビルドしてから `cargo package --locked --allow-dirty` ＋ `cargo package --list` による同梱内容の明示検証で保証する（dist の git 追跡は導入しない）。`--out` は same-dir temp + rename。server task は `serve_session` の `select!` に参加させ、異常終了は専用 exit code。headers は axum middleware 一枚。

**Tech Stack:** GitHub Actions, Rust (axum/tower middleware), tempファイル+rename。

## Global Constraints

- 全PRはmainベース・`gh pr create`経由（ローカルマージ禁止、`--amend`禁止）。`frontend/dist` は gitignore のまま（追跡しない）
- 新 exit code: `OUT_FAILED = 15`（`--out` 書き込み失敗）、`SERVER_FAILED = 16`（HTTP server 異常終了）。既存 0-3/10-14 は不変
- result JSON 契約・HTTP API 契約は不変（headers の追加のみ）
- branch protection（required checks 化）は**変更しない**（リモートCIに課金起因の startup_failure 歴があり、required 化はマージ自体をロックするリスクがある — PR 本文で推奨として言及するに留める）
- Push前ローカルCI必須。テスト実行は sonnet subagent へ委譲

---

### Task 1: CI 再構成と `cargo package` の green 化

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `Cargo.toml`（`rust-version = "1.82"` もここで追加。package メタデータの注記コメント更新）

**設計（P0-3 の根本原因と対処）:**
`frontend/dist/**` は `Cargo.toml` の `include` に入っているが git では ignore されているため、`cargo package --locked` は「packaged files が未コミット」として**どの環境でも必ず**失敗する（レビュー当初の推測と異なり、クロスプラットフォーム非決定性ではない）。dist を git 追跡に切り替えるのは生成物のコミット運用を常時強いるため採らず、package ジョブ側で検証を明示化する:

```yaml
jobs:
  rust:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings
      - name: Test
        run: cargo test

  package:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build frontend dist
        run: cargo build
      - name: Drop node_modules (keeps dependency licenses out of the crate)
        run: rm -rf frontend/node_modules
      - name: Package
        # frontend/dist is a gitignored build artifact deliberately shipped in
        # the crate (see Cargo.toml include), so the VCS dirty-check cannot
        # pass; --allow-dirty is required and the content check below is the
        # real gate.
        run: cargo package --locked --allow-dirty
      - name: Verify crate contents
        run: |
          cargo package --list --allow-dirty > /tmp/package-files.txt
          grep -q '^frontend/dist/index\.html$' /tmp/package-files.txt
          grep -q '^frontend/dist/assets/.*\.js$' /tmp/package-files.txt
          grep -q '^frontend/dist/assets/.*\.css$' /tmp/package-files.txt
          if grep -q 'node_modules' /tmp/package-files.txt; then
            echo 'node_modules leaked into the crate' >&2; exit 1
          fi

  frontend:
    # 既存のまま
```

補足:
- `cargo package` は `--allow-dirty` でも生成 crate をクリーン環境で再ビルド検証する（published crate 側は `frontend/package.json` が include に無いため build.rs は「dist 必須・ビルドしない」経路に入る＝この検証が `cargo install` 契約のテストになる）
- `Cargo.toml` に `rust-version = "1.82"`（`Option::is_none_or` 使用のため。Rust 1.82 で安定化）
- **Step: ローカルで package ジョブ相当を先に再現し（`cargo build && rm -rf frontend/node_modules && cargo package --locked --allow-dirty && cargo package --list --allow-dirty`）、green を確認してから ci.yml を書く**（node_modules を消すので終了後 `cd frontend && npm ci` で復元）

- [ ] Step 1: ローカルで package 手順を再現し成功を確認（失敗したら原因を特定して報告 — ここが本 PR の核）
- [ ] Step 2: ci.yml を上記構成へ書き換え、`Cargo.toml` に rust-version + コメント更新
- [ ] Step 3: `cargo test` 緑（Cargo.toml 変更の影響確認）→ Commit `ci: green cargo package via explicit content verification, fail-fast off, msrv 1.82`

---

### Task 2: atomic `--out`・server 異常終了の伝播・BadBase 分離

**Files:**
- Modify: `src/main.rs`（`exitcode::OUT_FAILED = 15`, `exitcode::SERVER_FAILED = 16`）
- Modify: `src/review.rs`（`serve_session`）
- Modify: `src/gitdiff.rs`（`rev_parse_commit` の失敗種別分離）
- Test: `tests/review_flow.rs`

**仕様:**
1. **atomic --out**: `std::fs::write(path, json)` を「同一ディレクトリの temp ファイル（`{filename}.tmp.{pid}`）へ write → `File::sync_all` 相当の flush → `std::fs::rename(temp, path)`」に変更。ポーリングしている統合側が partial JSON を読む race を排除。書き込み失敗時は stderr にエラーを出し **exit code 15**（stdout の result JSON は既に出力済みでよい — stdout 契約は維持し、exit code だけ OUT_FAILED にする。decision による 0/1 より優先）
2. **server 異常終了**: `serve_session` の `server_handle` を fire-and-forget にせず、outcome 待ち `select!` に branch として参加させる。server task が outcome より先に終了（accept loop エラー等）した場合は stderr にエラーを出し **exit code 16**。正常系（outcome 確定後の graceful shutdown）は従来通り
3. **BadBase 分離**: `rev_parse_commit` を「git を実行できなかった/異常終了以外の失敗（spawn error 等）」と「git は正常に実行されたが ref が解決できなかった」に分離し、前者は `GitError::GitFailed`、後者のみ `GitError::BadBase` に。既存の `bad_base_is_distinguished` テストは成立し続けること

**テスト**（`tests/review_flow.rs` の既存パターンを流用）:
- `--out` 先に書き込めないパス（存在しないディレクトリ配下）を指定 → submit 完了後 exit code 15、stdout には正しい result JSON
- `--out` 正常系: 出力ファイルが valid JSON（既存テストがあれば atomicity の回帰として temp ファイルが残っていないことも assert）

- [ ] Step 1: failing tests → Step 2: 実装 → Step 3: `cargo test` 緑 → Commit `fix: atomic --out with dedicated exit code and server failure propagation`

---

### Task 3: security / cache headers

**Files:**
- Modify: `src/server.rs`（middleware 追加）
- Test: `src/server.rs` tests

**仕様**（axum `middleware::from_fn` 一枚。パスで分岐）:
- 全レスポンス共通: `Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`
- `/assets/*`: `Cache-Control: public, max-age=31536000, immutable`（Vite の content-hash 付きファイル名前提）
- それ以外（index / API）: `Cache-Control: no-store`（token が URL に入るため）と
  `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'`
  （`style-src` の `'unsafe-inline'` は Svelte の style: ディレクティブ等が inline style 属性を生成し得るための現実解。コメントで理由を明記）

**テスト**: 既存の `call`/`get` ヘルパーで、(1) `/api/{token}/session` に no-store・nosniff・no-referrer・CSP が付く、(2) `/assets/...`（既存 `get_asset_serves_embedded_file` を拡張）に immutable Cache-Control が付く、(3) CSP に `frame-ancestors 'none'` を含む、を assert。

**動作確認**: headers 追加後に `ronten demo --no-open` をローカル起動し、ブラウザ相当で UI が CSP 違反で壊れていないか確認する（`curl` で index/assets の headers 確認＋frontend build 済み asset の読み込みが `'self'` で完結していることを確認。console エラーの実機確認は最終 Task 5 の browser 確認で行う）。

- [ ] Step 1: failing tests → Step 2: 実装 → Step 3: `cargo test` 緑 → Commit `feat: security and cache headers on all responses`

---

### Task 4: dirty worktree 警告・README 同期

**Files:**
- Modify: `src/review.rs`（起動時警告）
- Modify: `src/gitdiff.rs`（`has_tracked_changes` ヘルパー — 実装は下記）
- Modify: `README.md`
- Test: `tests/review_flow.rs`（警告の有無）

**仕様:**
1. `ronten review` 起動時（diff 計算後、serve 前）に hardened `git_cmd` で `status --porcelain -uno` を実行し、出力が非空なら stderr に1行警告:
   `warning: tracked files have uncommitted changes; this review covers committed state only (<base>...HEAD)`
   git status 自体の失敗は警告スキップ（レビュー続行、fail-open で可 — 表示専用機能のため）
2. README: exit code 表に 15 (`out write failed`) / 16 (`server failed`) を追加、`--out` の atomic 書き込み（temp+rename、poll 安全）を統合パターン節に反映、dirty worktree 警告の説明、CI/packaging の方針（dist は生成物、crate 同梱は package ジョブの content check で保証）を開発者向け節に追記

- [ ] Step 1: failing test（fixture repo で tracked ファイルを書き換えた状態で起動 → stderr に警告 / clean なら警告なし。review_flow の既存 spawn パターン利用）→ Step 2: 実装 → Step 3: README → Step 4: `cargo test` 緑 → Commit `feat: warn when tracked worktree changes are excluded from review` ＋ `docs: document new exit codes, atomic --out, and packaging policy`

---

### Task 5: 最終検証 → PR → マージ

- [ ] ローカルCI一式＋package 手順再現（subagent）
- [ ] `ronten demo` を起動し Browser で実 UI 確認（CSP で壊れていないか、console エラーなし、opaque ack・unmapped ハイライトの目視）
- [ ] Codex レビュー → 指摘対応
- [ ] push → `gh pr create --base main` → リモートCI で package ジョブ green を確認 → マージ

## 備考

- branch protection の required checks 化は行わない（課金起因 startup_failure でマージがロックされるリスク）。PR 本文で「課金安定後に package を required にする」ことを推奨として記載
- バージョン番号は 0.1.0 のまま（リリース作業自体はスコープ外）
