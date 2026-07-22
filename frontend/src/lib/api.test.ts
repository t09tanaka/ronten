import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { saveDraft, submit } from './api'
import type { Draft } from './types'

const draft: Draft = { concerns: {}, general_comments: ['hi'], acknowledgements: [] }

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

beforeEach(() => {
  // api.ts derives the token from the SPA path `/r/{token}`.
  vi.stubGlobal('location', { pathname: '/r/tok123' })
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('saveDraft', () => {
  it('sends the revision and mutation id alongside the draft and returns the new revision', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { revision: 4 }))
    vi.stubGlobal('fetch', fetchMock)

    const result = await saveDraft(draft, 3, 'mutation-a')

    expect(result).toEqual({ ok: true, revision: 4 })
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe('/api/tok123/draft')
    expect(init.method).toBe('PUT')
    expect(JSON.parse(init.body as string)).toEqual({
      revision: 3,
      draft,
      mutation_id: 'mutation-a',
    })
  })

  it('returns a draft conflict 409 as a result instead of throwing', async () => {
    const body = {
      error: 'draft conflict',
      current_revision: 7,
      details: ['the draft was changed elsewhere (another tab?)'],
    }
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(409, body)))

    const result = await saveDraft(draft, 3, 'mutation-a')

    expect(result).toEqual({ ok: false, ...body })
  })

  it('returns an already-submitted 409 as a result', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(409, { error: 'already submitted' })),
    )

    const result = await saveDraft(draft, 3, 'mutation-a')

    expect(result).toEqual({ ok: false, error: 'already submitted' })
  })

  it('throws on non-409 failures', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(413, { error: 'payload too large', details: [] })),
    )

    await expect(saveDraft(draft, 3, 'mutation-a')).rejects.toThrow('saveDraft failed: 413')
  })

  it('aborts a stalled request after the 40s timeout', async () => {
    vi.useFakeTimers()
    const fetchMock = vi.fn(
      (_url: string, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () =>
            reject(new DOMException('The operation was aborted.', 'AbortError')),
          )
        }),
    )
    vi.stubGlobal('fetch', fetchMock)

    const pending = saveDraft(draft, 3, 'mutation-a')
    const assertion = expect(pending).rejects.toThrow(/abort/i)
    await vi.advanceTimersByTimeAsync(40_000)
    await assertion
  })
})

describe('submit', () => {
  it('sends the revision and mutation id alongside the draft', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse(200, { ok: true }))
    vi.stubGlobal('fetch', fetchMock)

    const result = await submit(draft, 3, 'mutation-b')

    expect(result).toEqual({ ok: true })
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe('/api/tok123/submit')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body as string)).toEqual({
      revision: 3,
      draft,
      mutation_id: 'mutation-b',
    })
  })
})
