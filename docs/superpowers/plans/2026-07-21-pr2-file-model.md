# PR2: File Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 外部レビュー P1-4/6(部分)/7/8/9 への対応 — `FileDiff` を `change_kind`×`content_kind` の2軸＋mode/OID/size メタデータへ刷新し、opaque な変更（binary / non-UTF-8 / too-large）に明示 acknowledge を要求し、非UTF-8 パスを fail-closed にし、Bidi・不可視 Unicode をレビュー画面で可視化する。

**Architecture:** Rust 側は `src/gitdiff.rs` の `FileStatus` を廃止して `ChangeKind`（added/deleted/modified/renamed/copied）と `ContentKind`（text/binary/non-utf8/too-large）に分離、`RawEntry` が既に持つ mode/OID と `--batch-check` の size を `FileDiff` に載せる。ack は `Draft` に `acknowledged_opaque: Vec<usize>`（file index）を追加し submit 時にサーバー検証。フロントは types 更新・opaque カードの詳細表示＋ack チェックボックス・`revealInvisibles` による不可視文字の可視化と `unicode-bidi: isolate`。

**Tech Stack:** Rust (serde), Svelte 5 runes + TypeScript, vitest。

## Global Constraints

- 全PRはmainベース・`gh pr create`経由（ローカルマージ禁止、`--amend`禁止）
- `frontend/dist` は gitignore されており非追跡。コミットするのは frontend ソースのみ
- **result JSON（出力契約）は変更しない**。session payload（`files[]`・`draft`）はフロント専用なので形を変えてよい
- wire 名: `change_kind` は `"added" | "deleted" | "modified" | "renamed" | "copied"`、`content_kind` は `"text" | "binary" | "non-utf8" | "too-large"`（PR1 の `"non-utf8"` 表記を踏襲）
- Push前にローカルCI（fmt / clippy -D warnings / cargo test / frontend check+test+build）。テスト・lint 実行は sonnet subagent に委譲
- リモートCIの Package ステップ赤は既知（PR4対応）。ローカル検証を green 根拠とする

---

### Task 1: Rust `FileDiff` モデル刷新

**Files:**
- Modify: `src/gitdiff.rs`（`FileStatus` 廃止 → `ChangeKind`/`ContentKind`、`FileDiff` 拡張、`compute_diff`、`parse_unified_diff`、既存テスト全更新）
- Modify: `src/mapping.rs`（テストの `fd()` ヘルパーの `status` フィールド）
- Modify: `src/server.rs`（テスト内 `FileStatus` 参照があれば更新）
- Modify: `src/session.rs`（`FileDiff` 利用箇所はフィールド名変更の追従のみ）

**Interfaces:**
- Produces（後続タスクが依存する正確な形）:

```rust
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind { Added, Deleted, Modified, Renamed, Copied }

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum ContentKind {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "non-utf8")]
    NonUtf8,
    #[serde(rename = "too-large")]
    TooLarge,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change_kind: ChangeKind,
    pub content_kind: ContentKind,
    /// Git file mode（例 "100644", "100755", "120000", "160000"）。
    /// 存在しない側（added の old / deleted の new、mode "000000"）は None。
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// フル OID。zero-oid の側は None。`parse_unified_diff` 経由（demo）は常に None。
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    /// blob サイズ（bytes）。gitlink・存在しない側・不明（demo 経由）は None。
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// 内容が描画されない変更（binary / non-utf8 / too-large）。
    /// submit 時に明示 acknowledge が必要（Task 3）。
    pub fn is_opaque(&self) -> bool {
        self.content_kind != ContentKind::Text
    }
}
```

- [ ] **Step 1: 変換ルールを実装**（このタスクは既存テストの大規模な機械的更新を伴うため、変換ルールを先に固定し、テストは既存のものを新モデルへ書き換えて GREEN を保つ方式。新規挙動には新テストを書く）

`compute_diff` 内の対応:

| 旧 | 新 |
|---|---|
| `FileStatus::Added/Deleted/Modified/Renamed`（= `entry_status`） | `change_kind`: raw status `A`→Added, `D`→Deleted, `R`→Renamed, `C`→Copied, その他（`M`/`T` 等）→Modified |
| `FileStatus::Binary` | `content_kind: Binary`（`change_kind` は entry から） |
| `FileStatus::NonUtf8` | `content_kind: NonUtf8` |
| `FileStatus::TooLarge` | `content_kind: TooLarge` |
| `Plan::NoContent`（oid 同一 = pure rename / mode-only） | `content_kind: Text` + hunks 空。mode は載るので UI 側で「mode changed」表示可能に |

メタデータ移送:
- `old_mode`/`new_mode`: `RawEntry.old_mode`/`new_mode` が `"000000"` なら `None`、それ以外は `Some`
- `old_oid`/`new_oid`: `is_zero_oid` なら `None`
- `old_size`/`new_size`: `blob_sizes` の結果から。**サイズ取得対象を拡大**: 現在 `entry.old_oid == entry.new_oid` の entry は size_oids から除外されているが、mode-only 変更でもサイズ表示できるよう、gitlink でない非 zero-oid はすべて `--batch-check` に含める（`--batch-check` は内容を読まないので安価）。gitlink 側は `None`
- `parse_unified_diff`（demo 用）: `change_kind` は従来のヘッダ判定（`new file mode`→Added 等）、`content_kind` は Binary マーカー行で `Binary`、それ以外 `Text`。`new file mode <m>` / `deleted file mode <m>` の mode はそれぞれ `new_mode`/`old_mode` に `Some(m)` で格納、他は `None`。oid/size は常に `None`

- [ ] **Step 2: 新挙動の追加テスト**（`mod git_tests`）

```rust
#[test]
fn mode_only_change_exposes_modes() {
    let td = base_repo();
    let d = td.path();
    git(d, &["update-index", "--chmod=+x", "a.txt"]);
    git(d, &["commit", "-m", "make executable"]);

    let out = compute_diff(d, "main").unwrap();
    let a = find(&out.files, "a.txt");
    assert_eq!(a.change_kind, ChangeKind::Modified);
    assert_eq!(a.content_kind, ContentKind::Text);
    assert!(a.hunks.is_empty());
    assert_eq!(a.old_mode.as_deref(), Some("100644"));
    assert_eq!(a.new_mode.as_deref(), Some("100755"));
    assert_eq!(a.old_oid, a.new_oid);
    assert!(a.old_oid.is_some());
    assert!(a.old_size.is_some(), "size must be fetched even for equal-oid entries");
}

#[test]
fn binary_file_exposes_oids_and_sizes() {
    let td = base_repo();
    let d = td.path();
    std::fs::write(d.join("blob.bin"), b"\x00\x01\x02text").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-m", "binary"]);

    let out = compute_diff(d, "main").unwrap();
    let f = find(&out.files, "blob.bin");
    assert_eq!(f.content_kind, ContentKind::Binary);
    assert_eq!(f.change_kind, ChangeKind::Added);
    assert_eq!(f.old_oid, None);
    assert!(f.new_oid.is_some());
    assert_eq!(f.new_size, Some(7));
    assert_eq!(f.new_mode.as_deref(), Some("100644"));
}
```

- [ ] **Step 3: 既存テストの書き換え**

既存アサーション `assert_eq!(f.status, FileStatus::X)` は上表に従い `change_kind`/`content_kind` のアサーションへ。`mapping.rs` テストの `fd()` ヘルパーと `server.rs` テストの `FileDiff` 構築は新フィールドで埋める（メタデータは `None` で可）。ヘルパー例:

```rust
// mapping.rs テスト内 fd() の構築部
FileDiff {
    old_path: Some(path.to_string()),
    new_path: Some(path.to_string()),
    change_kind: ChangeKind::Modified,
    content_kind: ContentKind::Text,
    old_mode: None, new_mode: None,
    old_oid: None, new_oid: None,
    old_size: None, new_size: None,
    hunks: ...,
}
```

- [ ] **Step 4: `cargo test` / `cargo fmt --all -- --check` / `cargo clippy --all-targets -- -D warnings` 全緑**

- [ ] **Step 5: Commit** `feat!: split FileDiff status into change_kind and content_kind with file metadata`

---

### Task 2: 非UTF-8 パスの fail-closed 化

**Files:**
- Modify: `src/gitdiff.rs`（`parse_raw_z` のパス変換）
- Test: 既存 `raw_tests` mod

**Interfaces:**
- Produces: `parse_raw_z` はパス token が UTF-8 でなければ `GitError::GitFailed`。以後パスの `from_utf8_lossy` は使用しない（`�` 衝突による別ファイル同一表示・anchor 曖昧化を排除）。

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn parse_raw_z_rejects_non_utf8_path() {
    let raw = b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0\x80path\0";
    let err = parse_raw_z(raw).unwrap_err();
    let GitError::GitFailed(msg) = err else { panic!("expected GitFailed") };
    assert!(msg.contains("non-UTF-8"), "message should explain: {msg}");
}

#[test]
fn parse_raw_z_rejects_non_utf8_second_path() {
    let raw = b":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R90\0ok.txt\0\x81new\0";
    assert!(parse_raw_z(raw).is_err());
}
```

- [ ] **Step 2: 実装** — パス token を `std::str::from_utf8(path_token)` で変換し、`Err` なら `malformed(&format!("non-UTF-8 path {:?} (ronten requires UTF-8 paths)", String::from_utf8_lossy(path_token)))`。meta token 側の `from_utf8_lossy` は git 出力の ASCII 部分なので現状維持で可（厳密化済み）。

- [ ] **Step 3: `cargo test` 緑 → Commit** `fix: reject non-UTF-8 paths instead of lossy-collapsing them`

---

### Task 3: opaque 変更の明示 acknowledge（サーバー側）

**Files:**
- Modify: `src/session.rs`（`Draft` に `acknowledged_opaque`、`validate_draft` に検証追加）
- Test: `src/server.rs` の tests mod

**Interfaces:**
- Produces:

```rust
pub struct Draft {
    #[serde(default)]
    pub concerns: HashMap<String, ConcernDraft>,
    #[serde(default)]
    pub general_comments: Vec<String>,
    /// content が描画されない file（FileDiff::is_opaque）の明示 acknowledge。
    /// 値は session payload の files[] における index。
    #[serde(default)]
    pub acknowledged_opaque: Vec<usize>,
}
```

- `validate_draft` 追加ルール（submit 時のみ、PUT /draft は従来通り lenient）:
  - `acknowledged_opaque` の各 index が `files.len()` 未満であること。違反: `"acknowledged_opaque: unknown file index {i}"`
  - `acknowledged_opaque` の index が opaque でない file を指す場合。違反: `"acknowledged_opaque: file {path} is not opaque"`（path は `new_path` fallback `old_path`）
  - すべての opaque file index が `acknowledged_opaque` に含まれること。違反: `"opaque change not acknowledged: {path}"`
- result JSON（`ResultOutput`）は変更しない

- [ ] **Step 1: Failing tests**（`src/server.rs` tests。既存の `build_state` 相当に opaque file を含む fixture を追加）

```rust
/// Binary ファイルを1つ含む state（c1 が whole-file location で claim）。
fn build_opaque_state() -> (Arc<SessionState>, tokio::sync::mpsc::Receiver<Outcome>) {
    let mut files = parse_unified_diff(MODIFIED);
    files.push(FileDiff {
        old_path: Some("logo.png".to_string()),
        new_path: Some("logo.png".to_string()),
        change_kind: ChangeKind::Modified,
        content_kind: ContentKind::Binary,
        old_mode: Some("100644".to_string()),
        new_mode: Some("100644".to_string()),
        old_oid: Some("1111111111111111111111111111111111111111".to_string()),
        new_oid: Some("2222222222222222222222222222222222222222".to_string()),
        old_size: Some(10),
        new_size: Some(20),
        hunks: Vec::new(),
    });
    let input = ConcernsInput {
        version: 1,
        summary: None,
        concerns: vec![
            Concern {
                id: "c1".to_string(),
                title: "All".to_string(),
                description: None,
                risk: Risk::Medium,
                locations: vec![
                    Location { path: "src/app.ts".to_string(), side: None, start: None, end: None },
                    Location { path: "logo.png".to_string(), side: None, start: None, end: None },
                ],
            },
        ],
    };
    let mapping = resolve_mapping(&files, &input);
    assert!(mapping.unmapped.is_empty());
    // …既存 build_state と同様に SessionState を構築して返す…
}

#[tokio::test]
async fn submit_without_opaque_ack_422() {
    let (state, _rx) = build_opaque_state();
    let app = build_router(state);
    let draft = json!({
        "concerns": { "c1": { "verdict": "approve", "comments": [] } },
        "general_comments": []
    });
    let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(body["details"].to_string().contains("logo.png"));
}

#[tokio::test]
async fn submit_with_opaque_ack_succeeds() {
    let (state, mut rx) = build_opaque_state();
    let app = build_router(state);
    let draft = json!({
        "concerns": { "c1": { "verdict": "approve", "comments": [] } },
        "general_comments": [],
        "acknowledged_opaque": [1]
    });
    let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(matches!(rx.recv().await.unwrap(), Outcome::Submitted(_)));
}

#[tokio::test]
async fn submit_ack_on_non_opaque_file_422() {
    let (state, _rx) = build_opaque_state();
    let app = build_router(state);
    let draft = json!({
        "concerns": { "c1": { "verdict": "approve", "comments": [] } },
        "general_comments": [],
        "acknowledged_opaque": [0, 1, 99]
    });
    let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
}
```

- [ ] **Step 2: 実装**（`validate_draft` の末尾に検証ブロック追加。opaque index 集合と ack 集合を `HashSet<usize>` で突き合わせ）
- [ ] **Step 3: `cargo test` 緑 → Commit** `feat: require explicit acknowledgement of opaque changes at submit`

---

### Task 4: フロントエンド — モデル追従・opaque 詳細カード・ack UI

**Files:**
- Modify: `frontend/src/lib/types.ts`（`FileStatus` 廃止 → `ChangeKind`/`ContentKind`/`FileDiff` 刷新、`Draft` に `acknowledged_opaque: number[]`）
- Modify: `frontend/src/lib/DiffView.svelte`（header badge・opaque 詳細カード・ack チェックボックス）
- Modify: `frontend/src/lib/state.svelte.ts`（ack 操作・gating）
- Modify: `frontend/src/App.svelte` / `frontend/src/lib/VerdictBar.svelte`（submit gating 追従。該当箇所は `rs.allReviewed` の利用箇所）
- Test: `frontend/src/lib/opaque.test.ts`（新規。カード用テキスト整形のユニットテスト）

**Interfaces:**
- Consumes: Task 1 の wire 形（`change_kind`/`content_kind`/mode/oid/size）、Task 3 の `acknowledged_opaque`
- Produces:

```ts
// types.ts
export type ChangeKind = 'added' | 'deleted' | 'modified' | 'renamed' | 'copied'
export type ContentKind = 'text' | 'binary' | 'non-utf8' | 'too-large'
export interface FileDiff {
  old_path: string | null
  new_path: string | null
  change_kind: ChangeKind
  content_kind: ContentKind
  old_mode: string | null
  new_mode: string | null
  old_oid: string | null
  new_oid: string | null
  old_size: number | null
  new_size: number | null
  hunks: Hunk[]
}
export interface Draft {
  concerns: Record<string, ConcernDraft>
  general_comments: string[]
  acknowledged_opaque: number[]
}
```

```ts
// state.svelte.ts に追加するメンバ
isOpaque(f: FileDiff): boolean            // f.content_kind !== 'text'
isAcked(fileIndex: number): boolean
toggleAck(fileIndex: number): void        // locked 時 no-op、変更後 scheduleSave()
get allOpaqueAcked(): boolean             // session.files の opaque index が全て ack 済み
```

- submit ボタンの有効条件は従来の `allReviewed` に `allOpaqueAcked` を AND する（`App.svelte`/`VerdictBar.svelte` の実際のゲート箇所を読んで反映。未達時の tooltip/説明文言: `Acknowledge all opaque changes to submit`）
- `Draft` 初期化箇所（`state.svelte.ts` の初期値と `load()`）で `acknowledged_opaque` が無いサーバー draft（旧 draft）にも `?? []` で耐える

- [ ] **Step 1: 新規 `frontend/src/lib/opaque.ts` + failing unit tests**

```ts
// opaque.ts — opaque カードの表示行を純関数で組み立てる（テスト可能に）
import type { FileDiff, ContentKind } from './types'

export function contentNote(kind: ContentKind): string {
  switch (kind) {
    case 'text':
      return ''
    case 'binary':
      return 'Binary file changed (content not displayed)'
    case 'non-utf8':
      return 'Non-UTF-8 file changed (content not displayed)'
    case 'too-large':
      return 'File too large to display (content not displayed)'
    default: {
      const exhaustive: never = kind
      return exhaustive
    }
  }
}

export interface OpaqueDetailRow {
  label: string
  value: string
}

/** mode/oid/size を "old → new"（片側は "—"）で並べる。null 同士の行は出さない。 */
export function opaqueDetails(f: FileDiff): OpaqueDetailRow[] {
  const rows: OpaqueDetailRow[] = []
  const pair = (a: string | null, b: string | null): string =>
    `${a ?? '—'} → ${b ?? '—'}`
  if (f.old_mode != null || f.new_mode != null) rows.push({ label: 'mode', value: pair(f.old_mode, f.new_mode) })
  if (f.old_oid != null || f.new_oid != null)
    rows.push({ label: 'oid', value: pair(f.old_oid && f.old_oid.slice(0, 12), f.new_oid && f.new_oid.slice(0, 12)) })
  if (f.old_size != null || f.new_size != null)
    rows.push({
      label: 'size',
      value: pair(
        f.old_size != null ? `${f.old_size.toLocaleString()} B` : null,
        f.new_size != null ? `${f.new_size.toLocaleString()} B` : null,
      ),
    })
  return rows
}
```

テスト（`opaque.test.ts`）: `contentNote` の3種文言、`opaqueDetails` が mode/oid(12桁切詰)/size 行を生成すること、両側 null の行が出ないこと、added（old 側 null）で `— → x` になること。

- [ ] **Step 2: DiffView のカード刷新**
  - hunk なしファイルのカード: `content_kind === 'text'` なら従来相当の説明（`renamed` → "File renamed (no content changes)"、mode 変更あり → "File mode changed"、それ以外 → "File changed (no content changes)"）
  - opaque（`content_kind !== 'text'`）なら: `contentNote` 見出し + `opaqueDetails` の definition list + ack チェックボックス `I acknowledge this change (content cannot be reviewed)` を表示。checked 状態は `rs.isAcked(fileIndex)`、変更で `rs.toggleAck(fileIndex)`。`rs.phase !== 'review'` では disabled
  - header badge: `file.status` 表示を `file.change_kind` に変更し、opaque の場合は `content_kind` バッジを併記（`status-` CSS クラス名は `kind-` に揃えて調整可）
- [ ] **Step 3: state.svelte.ts / VerdictBar / App の gating 実装**（上記 Interfaces のとおり。既存コードを読んで `allReviewed` の使用箇所すべてに `allOpaqueAcked` を反映）
- [ ] **Step 4: `npm run check && npm run test && npm run build` 全緑**
- [ ] **Step 5: Commit** `feat: opaque-change detail card with explicit acknowledgement gating`

---

### Task 5: 不可視 Unicode の可視化と Bidi 隔離

**Files:**
- Create: `frontend/src/lib/invisibles.ts`, `frontend/src/lib/invisibles.test.ts`
- Modify: `frontend/src/lib/HunkView.svelte`（diff 行 content の表示経路に適用＋CSS）

**Interfaces:**
- Produces:

```ts
// invisibles.ts
/** Trojan Source 系の制御文字・不可視文字を可視トークン ⟨U+XXXX⟩ に置換する。 */
const INVISIBLE_CODEPOINTS: readonly number[] = [
  0x202a, 0x202b, 0x202c, 0x202d, 0x202e, // LRE RLE PDF LRO RLO
  0x2066, 0x2067, 0x2068, 0x2069,         // LRI RLI FSI PDI
  0x200b, 0x200c, 0x200d, 0x2060,         // ZWSP ZWNJ ZWJ WJ
  0xfeff,                                  // ZWNBSP/BOM
  0x00ad,                                  // SOFT HYPHEN
]

export function revealInvisibles(s: string): string
export function hasInvisibles(s: string): boolean
```

置換表記は `⟨U+202E⟩` 形式（大文字 hex、4桁 0 埋め）。適用位置は **syntax highlight より前の plain content**（置換後の文字列を従来のエスケープ/ハイライト経路へ流す。これによりマーカー自体もエスケープ経路を通り XSS 安全性を保つ）。

- [ ] **Step 1: Failing unit tests**（`invisibles.test.ts`）

```ts
import { describe, expect, it } from 'vitest'
import { hasInvisibles, revealInvisibles } from './invisibles'

describe('revealInvisibles', () => {
  it('replaces RLO with a visible token', () => {
    expect(revealInvisibles('a‮b')).toBe('a⟨U+202E⟩b')
  })
  it('replaces zero-width space and BOM', () => {
    expect(revealInvisibles('x​y﻿')).toBe('x⟨U+200B⟩y⟨U+FEFF⟩')
  })
  it('replaces every listed isolate control', () => {
    expect(revealInvisibles('⁦⁧⁨⁩')).toBe('⟨U+2066⟩⟨U+2067⟩⟨U+2068⟩⟨U+2069⟩')
  })
  it('leaves normal text (including CJK and emoji) untouched', () => {
    expect(revealInvisibles('日本語 emoji 🎉 tab\t')).toBe('日本語 emoji 🎉 tab\t')
  })
  it('hasInvisibles detects and rejects accordingly', () => {
    expect(hasInvisibles('plain')).toBe(false)
    expect(hasInvisibles('a‮b')).toBe(true)
  })
})
```

- [ ] **Step 2: 実装＋HunkView への適用**
  - `HunkView.svelte` の行 content 表示経路（highlight 呼び出しの入力）で `revealInvisibles(line.content)` を適用
  - diff 行の code セル相当の CSS に `unicode-bidi: isolate; direction: ltr;` を追加（コメント: Trojan Source 対策 — 表示順を論理順に固定）
- [ ] **Step 3: `npm run check && npm run test && npm run build` 全緑**
- [ ] **Step 4: Commit** `feat: reveal bidi and invisible unicode in diff lines`

---

### Task 6: ローカルCI → push → PR → マージ

- [ ] **Step 1:** ローカルCI一式（fmt / clippy / cargo test / frontend check+test+build）を subagent で最終確認
- [ ] **Step 2:** Codex レビュー（diff 全体＋新規フロント文言 `I acknowledge this change (content cannot be reviewed)` / `Acknowledge all opaque changes to submit` 等のトーン確認）→ 指摘対応
- [ ] **Step 3:** push → `gh pr create --base main` → マージ（事前承認済み）→ PR3 へ

## 備考（見送り・依存）

- `_unmapped` の changed-line 化・`deny_unknown_fields`・入力上限は PR3
- demo fixture（`fixtures/demo.diff`）は `parse_unified_diff` の新モデル対応で動き続ける（oid/size は None → opaque が無ければ ack UI は出ない）
- 旧 draft（`acknowledged_opaque` 無し）は serde default で空配列として読める。サーバー再起動をまたぐ draft 永続化は無いので互換は気にしなくてよいが、防御として `?? []` を入れる
