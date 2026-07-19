# ronten「朱入れ」デザイン刷新 — 設計スペック

日付: 2026-07-20
ステータス: 承認待ち

## 背景と目的

現行 UI は GitHub 風の汎用ライトテーマ（システムフォント・`#0969da` ブルー・GitHub diff 配色）で、
ronten 固有の個性がない。ronten（論点）の由来と「人間が агент の diff に朱を入れる」という
道具の本質に根ざしたビジュアルアイデンティティへ刷新する。

デザインの詳細規則は [docs/design-system.md](../../design-system.md) に定義した。
このスペックは決定事項とスコープの記録。

## 決定事項（ユーザー承認済み）

1. **方向性**: 「朱入れ」編集デザイン。白い紙 + 墨のテキスト、判定三色を伝統色に対応
   （request changes = 朱、approve = 松葉、comment = 藍）。diff の追加/削除行も同色体系に統一
2. **ダークモード**: 提供しない（ライト固定を維持）
3. **フォント**: Shippori Mincho の Latin サブセット woff2 を同梱（バイナリ +50KB 程度、OFL）。
   本文はシステムサンセリフ維持
4. **デザインシステム文書**: `docs/design-system.md` として恒久化（ユーザー要望）

## シグネチャ

- 落款印ロゴ（朱の正方形に白抜き「論」）をトップバーに
- レビュー済みマークを ✓ → 朱の ○ に。判定確定時の 120ms スタンプアニメーション
  （`prefers-reduced-motion` 尊重）が唯一のモーション

## スコープ

変更対象（すべて `frontend/`）:

- `src/app.css` — カラートークン定義・共有スタイル
- `src/App.svelte` — トップバー（落款印ロゴ）・モーダル・フッター・全体スタイル
- `src/lib/ConcernList.svelte` — 選択の朱縦罫・○マーク
- `src/lib/VerdictBar.svelte` — 判子風 3 連ボタン
- `src/lib/DiffView.svelte` / `src/lib/HunkView.svelte` — diff 配色の統一
- `src/lib/CommentEditor.svelte` — トークン参照化
- `src/assets/fonts/` — woff2 とライセンスファイルの追加（読み込みは `app.css` の
  `@font-face` で行うため `index.html` は変更しない）

## 非スコープ / 不変条件

- ロジック・状態管理・キーボード操作（`keynav.ts` / `state.svelte.ts`）は変更しない
- マークアップ構造の変更はロゴと○マークの追加など最小限
- レスポンシブ挙動・アクセシビリティ（フォーカス可視化）は劣化させない
- Rust 側（`src/`）は変更しない
- ダークモード・テーマ切替は追加しない

## 検証

- `ronten demo` を起動しブラウザ実機でスクリーンショット確認（通常状態・判定確定・
  コメント・モーダル・警告バナー・unmapped 論点）
- 既存フロントエンドのビルド（`cargo build` 経由）とテストが通ること
- キーボード操作（j/k/a/x/c/Enter/Esc）が従来どおり機能すること
