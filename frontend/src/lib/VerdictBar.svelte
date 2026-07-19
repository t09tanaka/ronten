<script lang="ts">
  import { rs } from './state.svelte'
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
      onclick={() => rs.setVerdict(concernId, opt.value)}
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
    padding: 6px 12px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    font-size: 13px;
    color: #333;
    cursor: pointer;
  }

  .verdict-btn:hover {
    background: #f0f1f3;
  }

  .verdict-btn.active.verdict-approve {
    background: #d6f5dd;
    border-color: #1a7f37;
    color: #1a7f37;
  }

  .verdict-btn.active.verdict-request-changes {
    background: #ffe0dd;
    border-color: #cf222e;
    color: #cf222e;
  }

  .verdict-btn.active.verdict-comment {
    background: #dbeafe;
    border-color: #0969da;
    color: #0969da;
  }
</style>
