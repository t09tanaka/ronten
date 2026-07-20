import type { Verdict } from './types'

/// Whether a verdict is "confirmed" — i.e. carries the information the
/// agent needs to act on it. Approve stands alone; request-changes and
/// comment verdicts stay unconfirmed until at least one comment exists,
/// either a line comment on the concern itself or a general comment on
/// the review. Only confirmed concerns count as reviewed, so submission
/// stays disabled until every verdict is confirmed.
export function isVerdictConfirmed(
  verdict: Verdict | null | undefined,
  concernCommentCount: number,
  generalCommentCount: number,
): boolean {
  if (!verdict) return false
  if (verdict === 'approve') return true
  return concernCommentCount > 0 || generalCommentCount > 0
}
