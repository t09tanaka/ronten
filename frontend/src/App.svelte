<script lang="ts">
  import { onMount } from 'svelte'
  import { rs } from './lib/state.svelte'
  import ConcernList from './lib/ConcernList.svelte'
  import DiffView from './lib/DiffView.svelte'

  onMount(() => {
    void rs.load()
  })

  // Tiny hand-rolled markdown converter for concern descriptions: paragraphs,
  // `- ` bullet lists, `code` spans, and fenced ``` code blocks. No external
  // markdown dependency. HTML is escaped first since descriptions come from
  // agent-supplied JSON.
  function escapeHtml(s: string): string {
    return s
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;')
  }

  function renderInline(s: string): string {
    return s.replace(/`([^`]+)`/g, '<code>$1</code>')
  }

  function renderMarkdown(src: string): string {
    const lines = escapeHtml(src).split('\n')
    const out: string[] = []
    let paragraph: string[] = []
    let list: string[] = []

    function flushParagraph(): void {
      if (paragraph.length > 0) {
        out.push(`<p>${renderInline(paragraph.join(' '))}</p>`)
        paragraph = []
      }
    }
    function flushList(): void {
      if (list.length > 0) {
        out.push(`<ul>${list.map((item) => `<li>${renderInline(item)}</li>`).join('')}</ul>`)
        list = []
      }
    }

    let i = 0
    while (i < lines.length) {
      const line = lines[i]
      if (line.startsWith('```')) {
        flushParagraph()
        flushList()
        const codeLines: string[] = []
        i++
        while (i < lines.length && !lines[i].startsWith('```')) {
          codeLines.push(lines[i])
          i++
        }
        out.push(`<pre><code>${codeLines.join('\n')}</code></pre>`)
        i++ // skip closing fence
        continue
      }
      if (line.startsWith('- ')) {
        flushParagraph()
        list.push(line.slice(2))
        i++
        continue
      }
      if (line.trim() === '') {
        flushParagraph()
        flushList()
        i++
        continue
      }
      flushList()
      paragraph.push(line.trim())
      i++
    }
    flushParagraph()
    flushList()
    return out.join('')
  }

  const submitTitle = $derived(
    rs.allReviewed
      ? 'Coming in review flow'
      : `Every concern needs a verdict (${(rs.session?.concerns.length ?? 0) - rs.reviewedCount} remaining)`,
  )
</script>

{#if rs.phase === 'loading'}
  <div class="center-message">Loading…</div>
{:else if rs.phase === 'error'}
  <div class="center-message">Session not found or already finished.</div>
{:else if rs.session}
  <div class="app">
    <header class="topbar">
      <div class="topbar-title">
        <h1>{rs.session.title}</h1>
        {#if rs.session.summary}
          <p class="summary">{rs.session.summary}</p>
        {/if}
      </div>
      <div class="topbar-actions">
        <span class="reviewed-counter"
          >{rs.reviewedCount}/{rs.session.concerns.length} reviewed</span
        >
        <button type="button" disabled title={submitTitle}>Submit review</button>
        <button type="button" disabled title="Coming in review flow">Abort review</button>
      </div>
    </header>
    <div class="body">
      <aside class="left-pane">
        <ConcernList />
      </aside>
      <main class="main-pane">
        {#if rs.selected}
          {@const selected = rs.selected}
          <div class="concern-header">
            <h2>{selected.title}</h2>
            {#if selected.risk}
              <span class="risk-badge risk-{selected.risk}">{selected.risk}</span>
            {/if}
            {#if selected.unmapped}
              <span class="unmapped-tag">unmapped</span>
            {/if}
          </div>
          {#if selected.description}
            <div class="concern-description">{@html renderMarkdown(selected.description)}</div>
          {/if}
          <DiffView />
        {/if}
      </main>
    </div>
  </div>
{/if}

<style>
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
    border-bottom: 1px solid #e2e2e2;
    background: #fafafa;
    flex-wrap: wrap;
  }

  .topbar-title h1 {
    margin: 0;
    font-size: 16px;
  }

  .topbar-title .summary {
    margin: 2px 0 0;
    font-size: 12px;
    color: #666;
    max-width: 60ch;
  }

  .topbar-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .reviewed-counter {
    font-size: 13px;
    color: #444;
    font-variant-numeric: tabular-nums;
  }

  .topbar-actions button {
    padding: 6px 12px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    font-size: 13px;
    color: #999;
    cursor: not-allowed;
  }

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .left-pane {
    width: 280px;
    flex: 0 0 280px;
    border-right: 1px solid #e2e2e2;
    overflow-y: auto;
    background: #fafafa;
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
    font-size: 18px;
  }

  .concern-description {
    font-size: 14px;
    line-height: 1.5;
    color: #333;
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
    background: #f0f0f0;
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 13px;
  }

  .concern-description :global(pre) {
    background: #f6f8fa;
    padding: 10px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .concern-description :global(pre code) {
    background: none;
    padding: 0;
  }
</style>
