// Pure helpers for the review-session deadline countdown (session.deadline_at
// — see session.rs). Kept free of any DOM/interval concerns so the math is
// unit-testable without faking timers or mounting a component.

/** Milliseconds remaining until `deadlineAt` (an RFC3339 timestamp), as of
 * `now` (epoch ms). Clamped to zero: an elapsed deadline and a
 * client/server clock skew that makes it look elapsed both just read as "0
 * left" rather than a negative number every caller would otherwise have to
 * guard against separately. An unparseable `deadlineAt` also reads as 0. */
export function remainingMs(deadlineAt: string, now: number): number {
  const deadline = new Date(deadlineAt).getTime()
  if (Number.isNaN(deadline)) return 0
  return Math.max(0, deadline - now)
}

/** Formats a millisecond duration as `m:ss` (minutes uncapped, seconds
 * zero-padded) for the countdown display. */
export function formatCountdown(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000)
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  return `${minutes}:${String(seconds).padStart(2, '0')}`
}
