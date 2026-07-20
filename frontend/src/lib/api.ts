import type { Draft, Session } from './types'

/**
 * The SPA is served at `/r/{token}`, so the token is the second path
 * segment: `''` / `'r'` / `'{token}'`.
 */
export function getToken(): string {
  return location.pathname.split('/')[2]
}

function apiUrl(path: string): string {
  return `/api/${getToken()}${path}`
}

export async function fetchSession(): Promise<Session> {
  const res = await fetch(apiUrl('/session'))
  if (!res.ok) {
    throw new Error(`fetchSession failed: ${res.status}`)
  }
  return (await res.json()) as Session
}

export async function saveDraft(draft: Draft): Promise<void> {
  const res = await fetch(apiUrl('/draft'), {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(draft),
  })
  if (!res.ok) {
    throw new Error(`saveDraft failed: ${res.status}`)
  }
}

export type SubmitResult =
  | { ok: true }
  | { error: string; missing?: string[]; details?: string[] }

export async function submit(draft: Draft): Promise<SubmitResult> {
  const res = await fetch(apiUrl('/submit'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(draft),
  })
  // Non-2xx responses (422/409) carry a JSON error body that callers need,
  // so they're parsed rather than thrown; only network failures throw.
  return (await res.json()) as SubmitResult
}

export async function abortSession(): Promise<void> {
  const res = await fetch(apiUrl('/abort'), { method: 'POST' })
  if (!res.ok) {
    throw new Error(`abortSession failed: ${res.status}`)
  }
}
