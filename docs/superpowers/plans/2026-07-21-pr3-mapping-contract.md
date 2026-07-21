# PR3: Mapping Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 外部レビュー P1-5/6・P2（契約系）への対応 — concern の claim を hunk 丸ごとから **changed line（add/remove）単位**へ変更して `_unmapped` の保証を強化し、v1 入力契約に `deny_unknown_fields` と入力サイズ上限を導入し、`request-changes` の理由必須をサーバー側で強制し、到達不能な `Decision::Abort` を出力スキーマから除去する。

**Architecture:** `resolve_mapping` を changed-line coverage ベースに再設計（display 用の hunk 割当は「そのhunk内で1行以上claimしたか」で決定、context 行の交差では claim しない）。`Mapping` に per-file の未claim changed line 集合を追加し session payload で `unmapped_lines` として配信、フロントは `_unmapped` 表示時に該当行をハイライト。入力側は serde `deny_unknown_fields` + 8MiB 上限。サーバー submit 検証に request-changes 理由必須を追加（フロントの既存 `isVerdictConfirmed` ルールと同一化）。

**Tech Stack:** Rust (serde/schemars), Svelte 5 + TS, vitest。

## Global Constraints

- 全PRはmainベース・`gh pr create`経由（ローカルマージ禁止、`--amend`禁止）。`frontend/dist` は gitignore（非追跡）
- **coverage 規則（正）**: location は changed line（add/remove）のみを claim する。context 行との交差は claim にならない
  - `side: "new"` → マッチした file の add 行（`new_no` が範囲内）
  - `side: "old"` → マッチした file の remove 行（`old_no` が範囲内）
  - `side` 未指定 → 同一範囲で add（`new_no`）と remove（`old_no`）の両方を claim（path マッチングは従来通り new 側優先のまま）
  - `start`/`end` 未指定 → whole-file（そのファイルの該当 side の全 changed line）
  - hunk なしファイル（opaque / no-content）は従来通り whole-file location（start/end なし）でのみ claim 可
- **display 規則**: concern の hunk 表示は「そのhunkで1行以上claimした」場合。`_unmapped` は「未claimのchanged lineを1行以上含むhunk」＋未claimのhunkなしファイル
- **warning 文言変更**: `location matched no hunks:` → `location matched no changed lines:`（範囲表記は従来形式を維持）
- result JSON: `Decision` から `abort` を除去する以外は不変
- Push前ローカルCI必須（fmt / clippy -D warnings / cargo test / frontend check+test+build）。テスト実行は sonnet subagent へ委譲。リモートCIの Package 赤は既知（PR4対応）

---

### Task 1: 入力契約の厳格化（deny_unknown_fields + 8MiB 上限）

**Files:**
- Modify: `src/model.rs`（`ConcernsInput`, `Concern`, `Location` に `#[serde(deny_unknown_fields)]`、モジュール doc 更新）
- Modify: `src/review.rs`（`read_concerns_source` にサイズ上限）
- Test: `src/model.rs` tests、`tests/cli.rs`

**Interfaces:**
- Produces: `pub const MAX_CONCERNS_BYTES: usize = 8 * 1024 * 1024;`（`review.rs`）。超過時は stderr にエラー＋exit `INPUT`。
- 既存テスト `parses_spec_example_and_ignores_unknown_fields` は逆転させる（unknown field はエラー）。

- [ ] **Step 1: Failing tests**

`src/model.rs` の既存テストを置き換え:

```rust
#[test]
fn rejects_unknown_fields_on_v1_input() {
    // version 1 しか受け付けない契約で unknown field を黙って無視すると、
    // 例えば "statr" のような typo が「ファイル全体claim」へ静かに拡大する。
    let top = r#"{"version":1, "bogus":1, "concerns":[{"id":"a","title":"t","risk":"low"}]}"#;
    assert!(serde_json::from_str::<ConcernsInput>(top).is_err());

    let concern = r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low","extra":1}]}"#;
    assert!(serde_json::from_str::<ConcernsInput>(concern).is_err());

    let location = r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low",
        "locations":[{"path":"a.ts","statr":120}]}]}"#;
    assert!(serde_json::from_str::<ConcernsInput>(location).is_err());

    let valid = r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low",
        "locations":[{"path":"a.ts","start":120}]}]}"#;
    assert!(serde_json::from_str::<ConcernsInput>(valid).is_ok());
}
```

`tests/cli.rs` に追加（既存の CLI テストのパターンに合わせる。fixture repo 構築ヘルパーが無い場合は `ronten review` が concerns 読み込みを repo 解決の後に行うことに注意 — 既存テストの構成を確認し、同等の方法で INPUT exit code を検証する）:

```rust
#[test]
fn oversized_concerns_input_is_rejected() {
    // 8MiB 超の concerns JSON は読み込み段階で拒否される（メモリ保護）。
    // 既存 cli.rs のテストと同様の起動方法で、--concerns に巨大ファイルを渡し
    // exit code 10 (INPUT) と stderr のサイズ超過メッセージを検証する。
}
```

- [ ] **Step 2: 実装**
  - `model.rs`: 3型に `#[serde(deny_unknown_fields)]`。モジュール docコメント「Unknown fields on input types are ignored…」を「rejected（version 固定契約のため。将来の拡張は version 2 で行う）」に書き換え。**注意**: `schemars` の derive が `deny_unknown_fields` を `additionalProperties: false` としてスキーマに反映することを `ronten schema --input` の出力で確認（反映されない場合は `#[schemars(deny_unknown_fields)]` 等の明示指定を調べて付与）
  - `review.rs` `read_concerns_source`: 読み込み後に `raw.len() > MAX_CONCERNS_BYTES` なら `Err(io::Error::new(InvalidData, format!("concerns input exceeds {} bytes", MAX_CONCERNS_BYTES)))`。stdin 経路は `std::io::Read::take(MAX_CONCERNS_BYTES as u64 + 1)` で読み過ぎ自体も防ぐ
- [ ] **Step 3: `cargo test` 緑 → Commit** `feat!: reject unknown fields and oversized concerns input`

---

### Task 2: changed-line coverage への mapping 再設計

**Files:**
- Modify: `src/mapping.rs`（`resolve_mapping` 本体・`Mapping` 構造）
- Modify: `src/session.rs`（`SessionPayload` に `unmapped_lines`）
- Modify: `src/server.rs`（payload 組み立て・既存テストの期待値）
- Test: `src/mapping.rs` tests（大幅更新）

**Interfaces:**
- Produces:

```rust
/// 未claimの changed line（`_unmapped` ハイライト用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UnmappedLine {
    pub file: usize,
    pub side: Side,   // Add 行は New、Remove 行は Old
    pub line: u32,    // side に対応する行番号（new_no / old_no）
}

pub struct Mapping {
    pub concerns: Vec<MappedConcern>,      // 形は従来通り（display 用 hunk 割当）
    pub unmapped: Vec<HunkRef>,            // 未claim行を含む hunk ＋ 未claimのhunkなしファイル
    pub unmapped_lines: Vec<UnmappedLine>, // 新規（file, side, line 昇順ソート）
    pub warnings: Vec<String>,
}
```

- `SessionPayload` に `pub unmapped_lines: &'a [UnmappedLine]` を追加（serde でそのまま配列に）
- パフォーマンス: location ループの前に `HashMap<&str, Vec<usize>>`（new_path 用と old_path 用）を構築し、全 file 線形走査をやめる（最大 200×200 locations に備える）

**アルゴリズム（この通りに実装）:**

```text
1. 前処理: file ごとに changed line 一覧を作る
   adds[file]    = [(hunk_idx, new_no)]   // kind == Add
   removes[file] = [(hunk_idx, old_no)]   // kind == Remove
2. claim 集合: claimed_adds: HashSet<(file, u32)>, claimed_removes: HashSet<(file, u32)>
   concern ごとの display 用: covered_hunks: BTreeSet<(file, Option<hunk>)>
3. 各 concern の各 location:
   - path マッチング（従来と同一: side=old なら old_path、それ以外は new_path。index 使用）
   - hunk なしファイル & start/end なし → HunkRef{hunk: None} を claim（従来通り、whole-file claim 集合へ）
   - hunk ありファイル:
     range = [start.unwrap_or(1), end.unwrap_or(u32::MAX)]
     side 指定 new → adds のみ / old → removes のみ / 未指定 → 両方
     マッチした changed line を claimed_* に追加し、その hunk を covered_hunks に追加
   - この location が changed line もhunkなしファイルも一切 claim しなかったら
     warning "location matched no changed lines: {path}{range表記}"
4. concern の hunks = covered_hunks をソートした Vec<HunkRef>（従来の順序規則: (file, hunk) 昇順）
5. unmapped_lines = 全 adds/removes のうち claimed_* に無いもの
6. unmapped =
   - hunk なしファイルで whole-file claim されなかったもの (HunkRef{hunk: None})
   - unmapped_lines を1行以上含む hunk の HunkRef（重複なし、(file, hunk) 昇順）
```

- [ ] **Step 1: 既存テストの期待値を新規則で書き直し＋新規テスト**（主な変更点と新テストは以下。既存テストは規則変更に沿って翻訳する）
  - `whole_file_location_claims_every_hunk` → 変わらず成立（whole-file は全 changed line を claim → 両 hunk display）
  - `range_intersection_boundaries` → hunk(new 10-19) に対し location 19-30: **その範囲に add 行が実在する場合のみ** claim。テストの `fd()` ヘルパーは lines が空なので、**`fd()` を「hunk 範囲に対応する Add/Remove 行を自動生成する」形に拡張する**（例: new range の各行に Add、old range の各行に Remove を生成。context は生成しない）。これにより既存テストの意図（範囲交差）は概ね保たれる
  - context-only claim の禁止を直接検証する新テスト:

```rust
#[test]
fn context_only_intersection_does_not_claim() {
    // hunk: new 10..=16, ただし changed line は new 13 の Add 1行だけ
    // （他は context）。location 10-12 は context としか交差しない。
    let files = vec![fd_with_lines(
        "a.ts",
        10, 7, 10, 7,
        &[(LineKind::Add, None, Some(13))],           // changed
        &[10, 11, 12, 14, 15, 16],                    // context (両側同番とする)
    )];
    let inp = input(vec![concern("c1", vec![loc("a.ts", None, Some(10), Some(12))])]);
    let mapping = resolve_mapping(&files, &inp);
    assert!(mapping.concerns[0].hunks.is_empty());
    assert_eq!(mapping.warnings.len(), 1);
    assert!(mapping.warnings[0].contains("matched no changed lines"));
    // 唯一の changed line は未claim → hunk は unmapped、行も unmapped_lines に載る
    assert_eq!(mapping.unmapped, vec![HunkRef { file: 0, hunk: Some(0) }]);
    assert_eq!(
        mapping.unmapped_lines,
        vec![UnmappedLine { file: 0, side: Side::New, line: 13 }]
    );
}

#[test]
fn partially_claimed_hunk_reports_remaining_lines_unmapped() {
    // 1つの hunk に add new13 と add new15。location は 13 のみ claim。
    // hunk は concern に表示されるが、15 は unmapped_lines に残り、
    // hunk 自体も _unmapped に載る（未説明の変更が残っているため）。
    let files = vec![fd_with_lines(
        "a.ts",
        13, 3, 13, 3,
        &[(LineKind::Add, None, Some(13)), (LineKind::Add, None, Some(15))],
        &[14],
    )];
    let inp = input(vec![concern("c1", vec![loc("a.ts", None, Some(13), Some(13))])]);
    let mapping = resolve_mapping(&files, &inp);
    assert_eq!(mapping.concerns[0].hunks, vec![HunkRef { file: 0, hunk: Some(0) }]);
    assert_eq!(
        mapping.unmapped_lines,
        vec![UnmappedLine { file: 0, side: Side::New, line: 15 }]
    );
    assert_eq!(mapping.unmapped, vec![HunkRef { file: 0, hunk: Some(0) }]);
}

#[test]
fn unspecified_side_claims_both_adds_and_removes() {
    // remove old10 + add new10 の modification。side 未指定 loc 10-10 で両方 claim。
    let files = vec![fd_with_lines(
        "a.ts",
        10, 1, 10, 1,
        &[(LineKind::Remove, Some(10), None), (LineKind::Add, None, Some(10))],
        &[],
    )];
    let inp = input(vec![concern("c1", vec![loc("a.ts", None, Some(10), Some(10))])]);
    let mapping = resolve_mapping(&files, &inp);
    assert!(mapping.unmapped.is_empty());
    assert!(mapping.unmapped_lines.is_empty());
}

#[test]
fn old_side_location_claims_only_removes() {
    let files = vec![fd_with_lines(
        "a.ts",
        10, 1, 10, 1,
        &[(LineKind::Remove, Some(10), None), (LineKind::Add, None, Some(10))],
        &[],
    )];
    let inp = input(vec![concern(
        "c1",
        vec![loc("a.ts", Some(Side::Old), Some(10), Some(10))],
    )]);
    let mapping = resolve_mapping(&files, &inp);
    // add new10 が未claim
    assert_eq!(
        mapping.unmapped_lines,
        vec![UnmappedLine { file: 0, side: Side::New, line: 10 }]
    );
}
```

  - `fd_with_lines(path, old_start, old_count, new_start, new_count, changed: &[(LineKind, Option<u32>, Option<u32>)], context_lines: &[u32])` ヘルパーを新設（context は old_no=new_no=n の Context 行として生成、changed とあわせ行番号順に並べる）。既存 `fd()` は前述のとおり「範囲から changed 行を自動生成」する実装に変更し、既存テストを最小修正で生かす
  - hunk なしファイル系テスト（binary claim / unmapped）は従来通り成立することを確認
- [ ] **Step 2: 実装**（アルゴリズム節の通り。`resolve_mapping` の docコメントも新規則で全面書き換え）
- [ ] **Step 3: `src/server.rs`**: payload に `unmapped_lines: &state.mapping.unmapped_lines` を追加。既存 server テストで `_unmapped` の有無・件数の期待値が新規則で変わる場合は追従（`MODIFIED` fixture は全行 changed の hunk が多いため、概ね従来の結果を維持するはず — 変わった場合はその理由をレビュー可能な形でコミットメッセージに書く）
- [ ] **Step 4: `cargo test` 緑 → Commit** `feat!: claim concerns by changed lines, not whole hunks`

---

### Task 3: request-changes 理由必須（サーバー）＋ `Decision::Abort` 除去

**Files:**
- Modify: `src/session.rs`（`validate_draft`）、`src/model.rs`（`Decision`）
- Test: `src/server.rs`、`src/model.rs`

**Interfaces:**
- 検証規則（フロント `confirmation.ts` の `isVerdictConfirmed` と同一）: verdict が `request-changes` の concern は、**その concern へのコメントが1件以上 or general comment が1件以上**なければ 422。violation 文言: `concern {id:?}: request-changes requires a comment explaining the reason`
- `Decision` は `Approve | RequestChanges` の2値に（`Abort` 除去）。abort/timeout は今まで通り stdout JSON を出さないので出力契約上の実質変更なし。`ronten schema --output` から `abort` が消える

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn submit_request_changes_without_reason_422() {
    let (state, _rx) = build_state();
    let app = build_router(state);
    let draft = json!({
        "concerns": {
            "c1": { "verdict": "request-changes", "comments": [] },
            "c2": { "verdict": "approve", "comments": [] },
            "_unmapped": { "verdict": "approve", "comments": [] }
        },
        "general_comments": []
    });
    let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(body["details"].to_string().contains("request-changes requires a comment"));
}

#[tokio::test]
async fn submit_request_changes_with_general_comment_succeeds() {
    let (state, mut rx) = build_state();
    let app = build_router(state);
    let draft = json!({
        "concerns": {
            "c1": { "verdict": "request-changes", "comments": [] },
            "c2": { "verdict": "approve", "comments": [] },
            "_unmapped": { "verdict": "approve", "comments": [] }
        },
        "general_comments": ["fix the auth check"]
    });
    let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(matches!(rx.recv().await.unwrap(), Outcome::Submitted(_)));
}
```

既存テスト `submit_complete_emits_outcome` は c1 が request-changes かつ general_comments 有りなので成立し続ける。`submit_valid_anchors_200` は `_unmapped` が request-changes ＋ concern コメ): 有りで成立。（Task 2 の mapping 変更で `_unmapped` の構成が変わっていた場合はここまでに追従済みの前提）
- [ ] **Step 2: 実装＋`Decision::Abort` 除去**（`model.rs` の enum・関連テスト。`grep -rn "Abort" src/` で `Decision::Abort` 参照が他に無いことを確認 — `Terminal::Aborted` / `Outcome::Aborted` は別物なので触らない）
- [ ] **Step 3: `cargo test` 緑 → Commit** `feat!: enforce request-changes reasons server-side and drop unreachable abort decision`

---

### Task 4: フロントエンド — unmapped 行ハイライト

**Files:**
- Modify: `frontend/src/lib/types.ts`（`UnmappedLine`、`Session.unmapped_lines`）
- Modify: `frontend/src/lib/state.svelte.ts`（lookup ヘルパー）
- Modify: `frontend/src/lib/HunkView.svelte`・`frontend/src/lib/DiffView.svelte`（`_unmapped` concern 表示時に該当行を強調）
- Test: 既存 vitest スイートの型追従（新規ロジックのユニットテストは lookup ヘルパーに対して追加）

**Interfaces:**

```ts
export interface UnmappedLine { file: number; side: Side; line: number }
export interface Session { /* 既存 + */ unmapped_lines: UnmappedLine[] }
```

`state.svelte.ts` に追加:

```ts
/** _unmapped concern 表示時のみ非 null。 (file, side, line) の高速 lookup。 */
isUnmappedLine(file: number, side: Side, line: number | null): boolean
```

（実装は `$derived` の `Set<string>`（`` `${file}:${side}:${line}` ``）を作り参照。selected concern が `unmapped: true` のときのみ有効化）

- HunkView: 行レンダリング時、`_unmapped` concern 選択中かつ `isUnmappedLine(fileIndex, side, lineNo)`（add 行は new側番号、remove 行は old側番号）に該当する行へ強調クラス（例 `line-unmapped`。左ボーダー等、design-system の警告色を利用）を付与。context 行は対象外
- 補足表示: `_unmapped` の説明文（server 由来の description）はそのまま。UI 側で「highlighted lines are changes no concern claimed」の説明を card か legend で 1 行添える

- [ ] **Step 1: 型追従＋lookup ヘルパー（ユニットテスト付き）→ Step 2: HunkView/DiffView 実装 → Step 3: `npm run check && npm run test && npm run build` 緑 → Step 4: Commit** `feat: highlight unclaimed changed lines in the unmapped view`

---

### Task 5: ローカルCI → Codex レビュー → push → PR → マージ

- [ ] ローカルCI一式（subagent）
- [ ] Codex レビュー（mapping 再設計の正しさ・deny_unknown_fields の互換影響・新文言）→ 指摘対応
- [ ] push → `gh pr create --base main` → マージ → PR4 へ

## 備考

- README の `_unmapped` 記述（"unmapped hunk"）が changed-line 保証に強化されるため、README 更新は PR4 のリリース整備でまとめて行う
- `deny_unknown_fields` は agent 側の後方互換を破る（バージョン固定契約の明確化）。PR 本文に BREAKING として明記
