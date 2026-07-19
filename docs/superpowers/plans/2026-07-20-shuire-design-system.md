# 「朱入れ」デザイン刷新 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ronten のフロントエンドを「朱入れ」デザインシステム（docs/design-system.md）に全面移行する。

**Architecture:** `app.css` に CSS カスタムプロパティでトークンを定義し、全コンポーネントの `<style>` をトークン参照に書き換える。ロジック・状態管理・キーボード操作は不変更。マークアップ変更は落款印ロゴ・○マーク・kbd フッター・verdict サマリーのクラス付与のみ。

**Tech Stack:** Svelte 5 + Vite 6。フォントは Shippori Mincho の woff2 サブセット2つ（Latin + 「論」1文字）を `frontend/src/assets/fonts/` に同梱。

## Global Constraints

- 生の hex はトークン定義（`app.css` の `:root`）と、トークン化しない一部の派生色（hover 濃色等、本計画に明記した値）以外に書かない
- `keynav.ts` / `state.svelte.ts` / `api.ts` / `types.ts` / Rust 側は変更しない
- ダークモードは追加しない（`color-scheme: light` 維持）
- 判定語彙: Approve / Request changes / Comment（表記変更なし）
- 検証コマンド: `cd frontend && npm run check && npm run test`（sonnet サブエージェントに委譲可）

### 追加トークン（design-system.md の表に対する追補、Task 7 で文書へ反映）

| 変数 | 値 | 用途 |
|---|---|---|
| `--c-ink-3` | `#9B968A` | 行番号・無効状態の文字 |
| `--c-neutral-tint` | `#ECEAE1` | low リスクバッジ・ファイルステータス背景 |
| `--c-gutter` | `#FAF8F2` | diff ガター基調背景 |

---

### Task 1: フォント同梱と app.css トークン基盤

**Files:**
- Create: `frontend/src/assets/fonts/shippori-mincho-latin-600.woff2`
- Create: `frontend/src/assets/fonts/shippori-mincho-ron-600.woff2`
- Create: `frontend/src/assets/fonts/OFL.txt`
- Modify: `frontend/src/app.css`（全面書き換え）

**Interfaces:**
- Produces: CSS カスタムプロパティ `--c-paper` `--c-panel` `--c-ink` `--c-ink-2` `--c-ink-3` `--c-rule` `--c-shu` `--c-matsuba` `--c-ai` `--c-odo` `--c-shu-tint` `--c-shu-tint-2` `--c-matsuba-tint` `--c-matsuba-tint-2` `--c-ai-tint` `--c-odo-tint` `--c-neutral-tint` `--c-gutter`、フォント変数 `--font-display` `--font-body` `--font-mono`、グローバルクラス `.risk-badge` `.risk-high` `.risk-medium` `.risk-low` `.unmapped-tag` `.center-message`。Task 2〜6 はすべてこれらを参照する

- [ ] **Step 1: フォントをダウンロード**

```bash
mkdir -p frontend/src/assets/fonts
curl -sL -o frontend/src/assets/fonts/shippori-mincho-latin-600.woff2 \
  "https://fonts.gstatic.com/s/shipporimincho/v17/VdGDAZweH5EbgHY6YExcZfDoj0B4A9GW45sPxeymzw.woff2"
curl -sL -o frontend/src/assets/fonts/shippori-mincho-ron-600.woff2 \
  "https://fonts.gstatic.com/l/font?kit=VdGDAZweH5EbgHY6YExcZfDoj0B4A9Gm4ZEI5-6czPTg3A&skey=89518b36fa7f2a37&v=v17"
curl -sL -o frontend/src/assets/fonts/OFL.txt \
  "https://raw.githubusercontent.com/google/fonts/main/ofl/shipporimincho/OFL.txt"
ls -la frontend/src/assets/fonts/
```

Expected: latin が 20–60KB、ron が 1–3KB、OFL.txt が 4KB 程度。0 バイトのファイルがあれば失敗として中断。
ダウンロード後、`file frontend/src/assets/fonts/*.woff2` で `Web Open Font Format (Version 2)` と出ることを確認（HTML エラーページを保存していないかの検査）。

- [ ] **Step 2: app.css を全面書き換え**

```css
@font-face {
  font-family: 'Shippori Mincho';
  font-style: normal;
  font-weight: 600;
  font-display: swap;
  src: url('./assets/fonts/shippori-mincho-latin-600.woff2') format('woff2');
  unicode-range: U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA,
    U+02DC, U+2000-206F, U+20AC, U+2122, U+2212, U+FEFF, U+FFFD;
}

/* 落款印の「論」1文字だけの極小サブセット */
@font-face {
  font-family: 'Shippori Mincho';
  font-style: normal;
  font-weight: 600;
  font-display: swap;
  src: url('./assets/fonts/shippori-mincho-ron-600.woff2') format('woff2');
  unicode-range: U+8AD6;
}

:root {
  color-scheme: light;

  /* 基調色（紙と墨） */
  --c-paper: #fcfbf8;
  --c-panel: #f6f4ee;
  --c-ink: #211f1c;
  --c-ink-2: #6e6a61;
  --c-ink-3: #9b968a;
  --c-rule: #e5e1d6;

  /* 意味色（判定の三色 + 警告） */
  --c-shu: #c2401f;
  --c-matsuba: #42704e;
  --c-ai: #2e4c66;
  --c-odo: #9a6700;

  /* 淡色トーン */
  --c-shu-tint: #f9ece7;
  --c-shu-tint-2: #f3ddd4;
  --c-matsuba-tint: #ecf2ec;
  --c-matsuba-tint-2: #dde9dd;
  --c-ai-tint: #eaf0f5;
  --c-odo-tint: #fbf3e0;
  --c-neutral-tint: #eceae1;
  --c-gutter: #faf8f2;

  /* 書体 */
  --font-display: 'Shippori Mincho', 'Hiragino Mincho ProN', 'Yu Mincho', Georgia, serif;
  --font-body: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Hiragino Kaku Gothic ProN',
    Helvetica, Arial, sans-serif;
  --font-mono: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
}

html,
body {
  height: 100%;
}

body {
  margin: 0;
  font-family: var(--font-body);
  background: var(--c-paper);
  color: var(--c-ink);
}

#app {
  height: 100%;
}

:focus-visible {
  outline: 2px solid var(--c-ai);
  outline-offset: 1px;
}

.center-message {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100vh;
  color: var(--c-ink-2);
  font-size: 15px;
}

/* Shared badge styles used by both ConcernList and the concern header in
   App.svelte. */
.risk-badge {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 3px;
  text-transform: uppercase;
  font-weight: 600;
  letter-spacing: 0.04em;
}

.risk-high {
  background: var(--c-shu-tint);
  color: var(--c-shu);
}

.risk-medium {
  background: var(--c-odo-tint);
  color: var(--c-odo);
}

.risk-low {
  background: var(--c-neutral-tint);
  color: var(--c-ink-2);
}

.unmapped-tag {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 3px;
  background: var(--c-odo-tint);
  color: var(--c-odo);
  font-weight: 600;
  text-transform: uppercase;
}
```

（旧 app.css にあった `.center-message` の重複定義は App.svelte 側を正とする。app.css 側は
loading/error/submitted/aborted 画面用に残す — 上記が唯一の定義になる。）

- [ ] **Step 3: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS（型・svelte-check・vitest すべて green）

- [ ] **Step 4: Commit**

```bash
git add frontend/src/assets/fonts frontend/src/app.css
git commit -m "feat: add shuire design tokens and bundled Shippori Mincho subsets"
```

---

### Task 2: App.svelte — トップバー・モーダル・フッター

**Files:**
- Modify: `frontend/src/App.svelte`（マークアップ 4 箇所 + `<style>` 全面書き換え）

**Interfaces:**
- Consumes: Task 1 のトークン全般
- Produces: なし（他タスクから参照されない）

- [ ] **Step 1: マークアップ変更（4 箇所）**

(1) トップバータイトルに落款印を追加（`.topbar-title` の中身を置き換え）:

```svelte
<div class="topbar-title">
  <span class="seal" aria-hidden="true">論</span>
  <div class="topbar-text">
    <h1>{rs.session.title}</h1>
    {#if rs.session.summary}
      <p class="summary">{rs.session.summary}</p>
    {/if}
  </div>
</div>
```

(2) ボタンにクラス付与（disabled / onclick 等の属性は現状維持のまま class だけ追加）:

- トップバー「Submit review」→ `class="btn-primary"`
- トップバー「Abort review」→ `class="btn-ghost"`
- submit モーダルの「Submit review」→ `class="btn-primary"`、「Cancel」→ `class="btn-ghost"`
- abort モーダルの「Abort review」→ `class="btn-primary"`、「Cancel」→ `class="btn-ghost"`
- General comments の「Add comment」→ `class="btn-outline"`

(3) verdict サマリーの表示を意味色クラス付きに（`.vs-verdict` の span を置き換え）:

```svelte
<span class="vs-verdict vs-{rs.draft.concerns[c.id]?.verdict ?? 'none'}"
  >{verdictLabel(rs.draft.concerns[c.id]?.verdict)}</span
>
```

(4) ショートカットフッターを kbd 表記に（`.shortcut-hint` の中身を置き換え）:

```svelte
<footer class="shortcut-hint">
  <kbd>j</kbd>/<kbd>k</kbd> select · <kbd>a</kbd> approve · <kbd>x</kbd> request changes ·
  <kbd>c</kbd> comment · <kbd>Enter</kbd> submit · <kbd>i</kbd> comment box · <kbd>Esc</kbd> close
</footer>
```

- [ ] **Step 2: `<style>` を以下で全面置き換え**

```css
.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
}

.topbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--c-rule);
  background: var(--c-panel);
  flex-wrap: wrap;
}

.topbar-title {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.seal {
  flex-shrink: 0;
  width: 28px;
  height: 28px;
  border-radius: 2px;
  background: var(--c-shu);
  color: var(--c-paper);
  font-family: var(--font-display);
  font-size: 16px;
  font-weight: 600;
  display: grid;
  place-items: center;
  user-select: none;
}

.topbar-text h1 {
  margin: 0;
  font-family: var(--font-display);
  font-size: 17px;
  font-weight: 600;
  letter-spacing: 0.01em;
}

.topbar-text .summary {
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--c-ink-2);
  max-width: 60ch;
}

.topbar-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.reviewed-counter {
  font-size: 13px;
  color: var(--c-ink-2);
  font-variant-numeric: tabular-nums;
}

.btn-primary {
  padding: 6px 14px;
  border: 1px solid var(--c-shu);
  border-radius: 3px;
  background: var(--c-shu);
  color: #fff;
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
}

.btn-primary:hover:not(:disabled) {
  background: #a83619;
  border-color: #a83619;
}

.btn-primary:disabled {
  background: #e9e6dd;
  border-color: #e9e6dd;
  color: var(--c-ink-3);
  cursor: not-allowed;
}

.btn-ghost {
  padding: 6px 14px;
  border: 1px solid var(--c-rule);
  border-radius: 3px;
  background: transparent;
  color: var(--c-ink-2);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
}

.btn-ghost:hover:not(:disabled) {
  background: #efece3;
  color: var(--c-ink);
}

.btn-ghost:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-outline {
  padding: 6px 14px;
  border: 1px solid var(--c-ai);
  border-radius: 3px;
  background: var(--c-paper);
  color: var(--c-ai);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
}

.btn-outline:hover:not(:disabled) {
  background: var(--c-ai-tint);
}

.btn-outline:disabled {
  border-color: var(--c-rule);
  color: var(--c-ink-3);
  cursor: not-allowed;
}

.warnings-banner {
  padding: 8px 16px;
  background: var(--c-odo-tint);
  border-bottom: 1px solid #ead9ab;
  color: var(--c-odo);
}

.warnings-banner-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.warnings-banner-title {
  font-size: 12px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.warnings-banner-dismiss {
  flex-shrink: 0;
  border: none;
  background: none;
  color: var(--c-odo);
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  padding: 0 2px;
}

.warnings-banner-dismiss:hover {
  color: #6b4900;
}

.warnings-banner-list {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.warnings-banner-list li {
  font-size: 12px;
  line-height: 1.4;
}

.body {
  flex: 1;
  display: flex;
  min-height: 0;
}

.left-pane {
  width: 280px;
  flex: 0 0 280px;
  border-right: 1px solid var(--c-rule);
  overflow-y: auto;
  background: var(--c-panel);
}

.main-pane {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
  min-width: 0;
}

.concern-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.concern-header h2 {
  margin: 0;
  font-family: var(--font-display);
  font-size: 20px;
  font-weight: 600;
}

.concern-description {
  font-size: 14px;
  line-height: 1.55;
  color: var(--c-ink);
  margin-bottom: 16px;
  max-width: 80ch;
}

.concern-description :global(p) {
  margin: 0 0 10px;
}

.concern-description :global(ul) {
  margin: 0 0 10px 20px;
  padding: 0;
}

.concern-description :global(code) {
  background: var(--c-neutral-tint);
  padding: 1px 4px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 13px;
}

.concern-description :global(pre) {
  background: var(--c-panel);
  border: 1px solid var(--c-rule);
  padding: 10px;
  border-radius: 4px;
  overflow-x: auto;
}

.concern-description :global(pre code) {
  background: none;
  padding: 0;
  border: none;
}

.concern-comment-list {
  list-style: none;
  margin: 0 0 14px;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.concern-comment-list li {
  font-size: 13px;
  color: var(--c-ink);
  background: var(--c-ai-tint);
  border: 1px solid #cfdce8;
  border-radius: 4px;
  padding: 6px 10px;
}

.comment-loc {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--c-ink-2);
  margin-right: 6px;
}

.general-comments {
  margin-top: 32px;
  padding-top: 16px;
  border-top: 1px solid var(--c-rule);
  max-width: 80ch;
}

.general-comments h3 {
  margin: 0 0 10px;
  font-family: var(--font-display);
  font-size: 15px;
  font-weight: 600;
}

.general-comments textarea {
  width: 100%;
  box-sizing: border-box;
  font-family: inherit;
  font-size: 13px;
  padding: 8px;
  border: 1px solid var(--c-rule);
  border-radius: 4px;
  background: var(--c-paper);
  color: var(--c-ink);
  resize: vertical;
}

.general-comments > .btn-outline {
  margin-top: 8px;
}

.general-comment-list {
  list-style: none;
  margin: 14px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.general-comment-list li {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  font-size: 13px;
  color: var(--c-ink);
  background: var(--c-panel);
  border: 1px solid var(--c-rule);
  border-radius: 4px;
  padding: 8px 12px;
  white-space: pre-wrap;
}

.comment-delete {
  flex-shrink: 0;
  border: none;
  background: none;
  color: var(--c-ink-2);
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  padding: 0 2px;
}

.comment-delete:hover {
  color: var(--c-shu);
}

.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(33, 31, 28, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}

.modal-panel {
  background: var(--c-paper);
  border-radius: 6px;
  padding: 20px 24px;
  max-width: 440px;
  width: 90%;
  max-height: 80vh;
  overflow-y: auto;
  box-shadow: 0 8px 30px rgba(33, 31, 28, 0.18);
}

.modal-panel h2 {
  margin: 0 0 12px;
  font-family: var(--font-display);
  font-size: 17px;
  font-weight: 600;
}

.modal-panel p {
  margin: 0 0 12px;
  font-size: 14px;
  color: var(--c-ink);
}

.verdict-summary {
  list-style: none;
  margin: 0 0 14px;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 260px;
  overflow-y: auto;
}

.verdict-summary li {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  font-size: 13px;
  padding: 4px 0;
  border-bottom: 1px solid var(--c-rule);
}

.vs-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vs-verdict {
  flex-shrink: 0;
  color: var(--c-ink-2);
}

.vs-approve {
  color: var(--c-matsuba);
}

.vs-request-changes {
  color: var(--c-shu);
}

.vs-comment {
  color: var(--c-ai);
}

.modal-error {
  font-size: 13px;
  color: var(--c-shu);
  background: var(--c-shu-tint);
  border: 1px solid var(--c-shu-tint-2);
  border-radius: 4px;
  padding: 8px 10px;
  margin: 0 0 12px;
}

.modal-actions {
  display: flex;
  gap: 10px;
  justify-content: flex-end;
}

.shortcut-hint {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 4px 16px;
  font-size: 11px;
  color: var(--c-ink-2);
  background: rgba(252, 251, 248, 0.92);
  border-top: 1px solid var(--c-rule);
  text-align: center;
  pointer-events: none;
  z-index: 5;
}

.shortcut-hint kbd {
  font-family: var(--font-mono);
  font-size: 10px;
  padding: 1px 4px;
  border: 1px solid var(--c-rule);
  border-bottom-width: 2px;
  border-radius: 3px;
  background: #fff;
  color: var(--c-ink-2);
}
```

（旧 `<style>` 内の `.center-message` はコンポーネントから削除し、app.css のグローバル定義に一本化。
`.topbar-actions button` / `.modal-actions button` 系のタグセレクタは廃止し、上記のクラスベースに置き換え。）

- [ ] **Step 3: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add frontend/src/App.svelte
git commit -m "feat: restyle app shell with seal logo, shu buttons, kbd footer"
```

---

### Task 3: ConcernList.svelte — 朱縦罫と○マーク

**Files:**
- Modify: `frontend/src/lib/ConcernList.svelte`

**Interfaces:**
- Consumes: Task 1 のトークン、グローバル `.risk-badge` `.unmapped-tag`

- [ ] **Step 1: レビュー済みマークのマークアップ置き換え**

`.reviewed-check` の span を以下に置き換え:

```svelte
<span class="reviewed-mark" role="img" aria-label="reviewed" title="reviewed">
  <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true">
    <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" stroke-width="1.8" />
  </svg>
</span>
```

- [ ] **Step 2: `<style>` を以下で全面置き換え**

```css
.concern-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.concern-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 10px 14px 10px 11px;
  border: none;
  border-bottom: 1px solid var(--c-rule);
  border-left: 3px solid transparent;
  background: none;
  text-align: left;
  cursor: pointer;
  font: inherit;
  font-size: 13px;
  color: var(--c-ink);
}

.concern-row:hover {
  background: #efece3;
}

.concern-row.selected {
  border-left-color: var(--c-shu);
  background: var(--c-ai-tint);
}

.concern-row.unmapped {
  background: var(--c-odo-tint);
}

.concern-row.unmapped.selected {
  border-left-color: var(--c-shu);
  background: #f3e7c4;
}

.concern-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.concern-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.reviewed-mark {
  display: inline-flex;
  color: var(--c-shu);
  animation: stamp 120ms ease-out;
}

@keyframes stamp {
  from {
    transform: scale(1.25);
    opacity: 0;
  }
  to {
    transform: scale(1);
    opacity: 1;
  }
}

@media (prefers-reduced-motion: reduce) {
  .reviewed-mark {
    animation: none;
  }
}
```

- [ ] **Step 3: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/ConcernList.svelte
git commit -m "feat: restyle concern list with shu rule and maru stamp mark"
```

---

### Task 4: VerdictBar.svelte — 判定三色

**Files:**
- Modify: `frontend/src/lib/VerdictBar.svelte`（`<style>` のみ）

**Interfaces:**
- Consumes: Task 1 のトークン

- [ ] **Step 1: `<style>` を以下で全面置き換え**

```css
.verdict-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 14px;
}

.verdict-btn {
  padding: 6px 14px;
  border: 1px solid var(--c-rule);
  border-radius: 3px;
  background: var(--c-paper);
  font-size: 13px;
  font-family: inherit;
  color: var(--c-ink);
  cursor: pointer;
}

.verdict-btn:hover {
  background: var(--c-panel);
}

.verdict-btn.active.verdict-approve {
  background: var(--c-matsuba-tint);
  border-color: var(--c-matsuba);
  color: var(--c-matsuba);
}

.verdict-btn.active.verdict-request-changes {
  background: var(--c-shu-tint);
  border-color: var(--c-shu);
  color: var(--c-shu);
}

.verdict-btn.active.verdict-comment {
  background: var(--c-ai-tint);
  border-color: var(--c-ai);
  color: var(--c-ai);
}
```

- [ ] **Step 2: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/VerdictBar.svelte
git commit -m "feat: restyle verdict bar with semantic three-color states"
```

---

### Task 5: DiffView.svelte + HunkView.svelte — diff 配色統一

**Files:**
- Modify: `frontend/src/lib/DiffView.svelte`（`<style>` のみ）
- Modify: `frontend/src/lib/HunkView.svelte`（`<style>` のみ）

**Interfaces:**
- Consumes: Task 1 のトークン

- [ ] **Step 1: DiffView の `<style>` を以下で全面置き換え**

```css
.file-group {
  margin-bottom: 24px;
}

.file-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  background: var(--c-panel);
  border: 1px solid var(--c-rule);
  border-bottom: none;
  border-radius: 4px 4px 0 0;
  font-family: var(--font-mono);
  font-size: 13px;
}

.file-path {
  font-weight: 600;
}

.file-status {
  font-size: 11px;
  text-transform: uppercase;
  color: var(--c-ink-2);
  background: var(--c-neutral-tint);
  padding: 1px 6px;
  border-radius: 3px;
}

.status-card {
  padding: 10px;
  border: 1px solid var(--c-rule);
  border-top: none;
  font-size: 13px;
  color: var(--c-ink-2);
  font-style: italic;
}

.empty-diff {
  color: var(--c-ink-2);
  font-size: 14px;
}
```

- [ ] **Step 2: HunkView の `<style>` の色指定をトークンに置き換え**

レイアウト系（sticky ガター、container-query、`--gutter-w` の仕組み）は一切変更せず、
色・書体の宣言だけ以下の対応で置き換える:

| 旧 | 新 |
|---|---|
| `.hunk` の `border: 1px solid #e2e2e2` | `border: 1px solid var(--c-rule)` |
| `.hunk-header` の `background: #fafafa` / `color: #8b949e` / `border-bottom: 1px solid #eee` | `background: var(--c-panel)` / `color: var(--c-ink-3)` / `border-bottom: 1px solid var(--c-rule)` |
| `.hunk-header` の `font-family: ui-monospace, ...` | `font-family: var(--font-mono)` |
| `.hunk-range` の `color: #6e7781` | `color: var(--c-ink-3)` |
| `.hunk-section` の `color: #999` | `color: var(--c-ink-3)` |
| `.shared-badge` の `color: #9a6700` | `color: var(--c-odo)` |
| `.owner-link` の `color: #0969da` | `color: var(--c-ai)` |
| `.collapse-toggle` の `background: #f6f8fa` / `border-top: 1px solid #eee` / `color: #57606a` / `font-family: ui-monospace, ...` | `background: var(--c-panel)` / `border-top: 1px solid var(--c-rule)` / `color: var(--c-ink-2)` / `font-family: var(--font-mono)` |
| `.hunk-table` の `font-family: ui-monospace, ...` | `font-family: var(--font-mono)` |
| `.gutter` の `color: #8b949e` / `background: #fafbfc` | `color: var(--c-ink-3)` / `background: var(--c-gutter)` |
| `.new-gutter` の `box-shadow: inset -1px 0 0 #e2e2e2` | `box-shadow: inset -1px 0 0 var(--c-rule)` |
| `.line-add` の `background: #e6ffec` | `background: var(--c-matsuba-tint)` |
| `.line-remove` の `background: #ffebe9` | `background: var(--c-shu-tint)` |
| `.line-add .gutter` の `background: #ccffd8` | `background: var(--c-matsuba-tint-2)` |
| `.line-remove .gutter` の `background: #ffd7d5` | `background: var(--c-shu-tint-2)` |
| `.gutter:hover` の `background: #eaeef2` | `background: #edebe1` |
| `.comment-block` の `background: #fff8c5` / `border: 1px solid #d4c76a` / `color: #333` / `font-family: ui-sans-serif, ...` | `background: var(--c-ai-tint)` / `border: 1px solid #cfdce8` / `color: var(--c-ink)` / `font-family: var(--font-body)` |
| `.comment-delete` の `color: #666`、hover の `color: #cf222e` | `color: var(--c-ink-2)`、hover は `color: var(--c-shu)` |

- [ ] **Step 3: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/DiffView.svelte frontend/src/lib/HunkView.svelte
git commit -m "feat: unify diff colors with shuire palette"
```

---

### Task 6: CommentEditor.svelte

**Files:**
- Modify: `frontend/src/lib/CommentEditor.svelte`（`<style>` のみ + ボタンクラス付与）

**Interfaces:**
- Consumes: Task 1 のトークン

- [ ] **Step 1: マークアップ変更**

「Add comment」ボタンに `class="editor-add"`、「Cancel」に `class="editor-cancel"` を付与。

- [ ] **Step 2: `<style>` を以下で全面置き換え**

```css
.comment-editor {
  padding: 10px;
  background: var(--c-panel);
  border: 1px solid var(--c-rule);
  border-radius: 4px;
  margin: 4px 0;
}

.comment-editor textarea {
  width: 100%;
  box-sizing: border-box;
  font-family: inherit;
  font-size: 13px;
  padding: 8px;
  border: 1px solid var(--c-rule);
  border-radius: 4px;
  background: var(--c-paper);
  color: var(--c-ink);
  resize: vertical;
}

.comment-editor-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}

.editor-add {
  padding: 5px 12px;
  border: 1px solid var(--c-ai);
  border-radius: 3px;
  background: var(--c-paper);
  color: var(--c-ai);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
}

.editor-add:hover:not(:disabled) {
  background: var(--c-ai-tint);
}

.editor-add:disabled {
  border-color: var(--c-rule);
  color: var(--c-ink-3);
  cursor: not-allowed;
}

.editor-cancel {
  padding: 5px 12px;
  border: 1px solid var(--c-rule);
  border-radius: 3px;
  background: transparent;
  color: var(--c-ink-2);
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
}

.editor-cancel:hover {
  background: #efece3;
  color: var(--c-ink);
}
```

- [ ] **Step 3: 検証**

Run: `cd frontend && npm run check && npm run test`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/CommentEditor.svelte
git commit -m "feat: restyle comment editor with shuire tokens"
```

---

### Task 7: 統合検証・スクリーンショット確認・文書同期

**Files:**
- Modify: `docs/design-system.md`（追加トークン 3 つを表に追記）

- [ ] **Step 1: design-system.md のカラートークン表に追補を反映**

「淡色トーン」表に `--c-neutral-tint: #ECEAE1`（low バッジ・ステータス背景）と
`--c-gutter: #FAF8F2`（diff ガター）、「基調色」表に `--c-ink-3: #9B968A`（行番号・無効文字）を追加。

- [ ] **Step 2: フルビルドと全チェック**

Run: `cd frontend && npm run check && npm run test && npm run build`
Expected: すべて PASS、`dist/` にフォント 2 ファイルが assets として出力される

Run: `cargo build`
Expected: PASS（フロントエンド埋め込み込みでビルド成功）

- [ ] **Step 3: 実機スクリーンショット検証**

`./target/debug/ronten demo --port 7878 --no-open` を起動し、ブラウザで以下を確認:

- 通常表示（落款印・明朝見出し・紙背景）
- 判定 3 種の確定状態と○マークのスタンプ表示
- unmapped 論点の表示・警告バナー
- 行コメント追加（藍のインラインコメント）・general comment
- submit / abort モーダル（verdict サマリーの意味色）
- キーボード操作 j/k/a/x/c/Enter/Esc が全て機能すること

- [ ] **Step 4: Commit**

```bash
git add docs/design-system.md
git commit -m "docs: sync design system tokens with implementation"
```
