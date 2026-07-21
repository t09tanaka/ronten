import type { Draft, FinishedKind, SaveDraftResult, Session } from './types'

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

const FETCH_TIMEOUT_MS = 15_000

/** fetch with a 15s abort timeout — a stalled request must not leave the
 * UI stuck in a saving/submitting state forever. */
async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)
  try {
    return await fetch(apiUrl(path), { ...init, signal: controller.signal })
  } finally {
    clearTimeout(timer)
  }
}

export async function fetchSession(): Promise<Session> {
  const res = await apiFetch('/session')
  if (!res.ok) {
    throw new Error(`fetchSession failed: ${res.status}`)
  }
  return (await res.json()) as Session
}

export async function saveDraft(draft: Draft, revision: number): Promise<SaveDraftResult> {
  const res = await apiFetch('/draft', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ revision, draft }),
  })
  if (res.ok) {
    const body = (await res.json()) as { revision: number }
    return { ok: true, revision: body.revision }
  }
  // 409 carries a JSON error body the caller must inspect (draft conflict
  // vs session finished), so it's returned rather than thrown; anything
  // else is an ordinary failure.
  if (res.status === 409) {
    const body = (await res.json()) as {
      error: string
      current_revision?: number
      finished?: FinishedKind
      details?: string[]
    }
    return { ok: false, ...body }
  }
  throw new Error(`saveDraft failed: ${res.status}`)
}

export type SubmitResult =
  | { ok: true }
  | {
      error: string
      missing?: string[]
      details?: string[]
      current_revision?: number
      finished?: FinishedKind
    }

/** Submit carries the same revision handshake as a save: a stale tab must
 * not be able to submit past another tab's newer draft. */
export async function submit(draft: Draft, revision: number): Promise<SubmitResult> {
  const res = await apiFetch('/submit', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ revision, draft }),
  })
  // Non-2xx responses (422/409) carry a JSON error body that callers need,
  // so they're parsed rather than thrown; only network failures throw.
  return (await res.json()) as SubmitResult
}

export async function abortSession(): Promise<void> {
  const res = await apiFetch('/abort', { method: 'POST' })
  if (!res.ok) {
    throw new Error(`abortSession failed: ${res.status}`)
  }
}
