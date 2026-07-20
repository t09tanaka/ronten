<script lang="ts">
  import { rs } from './state.svelte'
  import { focusGeneralComments } from './scroll'
  import type { Verdict } from './types'

  interface Props {
    concernId: string
  }

  let { concernId }: Props = $props()

  const verdict = $derived(rs.draft.concerns[concernId]?.verdict ?? null)

  const options: { value: Verdict; label: string }[] = [
    { value: 'approve', label: 'Approve' },
    { value: 'request-changes', label: 'Request changes' },
    { value: 'comment', label: 'Comment' },
  ]
</script>

<div class="verdict-bar" role="group" aria-label="Verdict">
  {#each options as opt (opt.value)}
    <button
      type="button"
      class="verdict-btn verdict-{opt.value}"
      class:active={verdict === opt.value}
      aria-pressed={verdict === opt.value}
      onclick={() => {
        rs.setVerdict(concernId, opt.value)
        // Nudge toward writing the reason for non-approve verdicts.
        if (opt.value !== 'approve') focusGeneralComments()
      }}
    >
      {opt.label}
    </button>
  {/each}
</div>

<style>
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
</style>
