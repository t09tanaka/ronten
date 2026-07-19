<script lang="ts">
  import { rs } from './state.svelte'
</script>

<ul class="concern-list">
  {#each rs.session?.concerns ?? [] as concern, i (concern.id)}
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
          {#if rs.draft.concerns[concern.id]?.verdict}
            <span class="reviewed-mark" role="img" aria-label="reviewed" title="reviewed">
              <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" stroke-width="1.8" />
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
</style>
