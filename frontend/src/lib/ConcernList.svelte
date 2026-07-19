<script lang="ts">
  import { rs } from './state.svelte'
  import type { Verdict } from './types'

  const MARK_LABELS: Record<Verdict, string> = {
    approve: 'Approved',
    'request-changes': 'Changes requested',
    comment: 'Commented',
  }

  function verdictOf(id: string): Verdict | null {
    return rs.draft.concerns[id]?.verdict ?? null
  }
</script>

<ul class="concern-list">
  {#each rs.session?.concerns ?? [] as concern, i (concern.id)}
    {@const verdict = verdictOf(concern.id)}
    <li data-idx={i}>
      <button
        type="button"
        class="concern-row"
        class:selected={i === rs.selectedIdx}
        class:unmapped={concern.unmapped}
        onclick={() => rs.select(i)}
      >
        <span class="concern-title">{concern.title}</span>
        <span class="concern-meta">
          {#if concern.risk}
            <span class="risk-badge risk-{concern.risk}">{concern.risk}</span>
          {/if}
          {#if concern.unmapped}
            <span class="unmapped-tag">unmapped</span>
          {/if}
          {#if verdict}
            <span
              class="verdict-mark mark-{verdict}"
              role="img"
              aria-label={MARK_LABELS[verdict]}
              title={MARK_LABELS[verdict]}
            >
              <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true">
                {#if verdict === 'approve'}
                  <path
                    d="M2.5 7.5 L5.5 10.5 L11.5 3.5"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.8"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                {:else if verdict === 'request-changes'}
                  <path
                    d="M3.5 3.5 L10.5 10.5 M10.5 3.5 L3.5 10.5"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.8"
                    stroke-linecap="round"
                  />
                {:else}
                  <path
                    d="M2.5 3 h9 v6.5 h-5 l-2.5 2.2 v-2.2 h-1.5 z"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    stroke-linejoin="round"
                  />
                {/if}
              </svg>
            </span>
          {/if}
        </span>
      </button>
    </li>
  {/each}
</ul>

<style>
  .concern-list {
    list-style: none;
    margin: 0;
    /* Bottom clearance so the last row scrolls clear of the fixed
       shortcut-hint footer. */
    padding: 0 0 40px;
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
    background: var(--c-hover-wash);
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
    background: var(--c-odo-tint-2);
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

  .verdict-mark {
    display: inline-flex;
    animation: stamp 120ms ease-out;
  }

  .mark-approve {
    color: var(--c-matsuba);
  }

  .mark-request-changes {
    color: var(--c-shu);
  }

  .mark-comment {
    color: var(--c-ai);
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
    .verdict-mark {
      animation: none;
    }
  }
</style>
