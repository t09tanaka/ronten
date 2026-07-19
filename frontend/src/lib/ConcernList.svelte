<script lang="ts">
  import { rs } from './state.svelte'
</script>

<ul class="concern-list">
  {#each rs.session?.concerns ?? [] as concern, i (concern.id)}
    <li>
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
            <span class="reviewed-check" aria-label="reviewed" title="reviewed">✓</span>
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
    padding: 10px 14px;
    border: none;
    border-bottom: 1px solid #ececec;
    background: none;
    text-align: left;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    color: #1a1a1a;
  }

  .concern-row:hover {
    background: #f0f1f3;
  }

  .concern-row.selected {
    background: #e8f0fe;
  }

  .concern-row.unmapped {
    background: #fff9ee;
  }

  .concern-row.unmapped.selected {
    background: #ffe9c7;
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

  .reviewed-check {
    color: #1a7f37;
    font-weight: 700;
  }
</style>
