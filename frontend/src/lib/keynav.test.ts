import { describe, expect, it } from 'vitest'
import { interpretKey, type KeyInput } from './keynav'

function input(overrides: Partial<KeyInput> & Pick<KeyInput, 'key'>): KeyInput {
  return {
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    isComposing: false,
    targetTag: 'BODY',
    targetEditable: false,
    ...overrides,
  }
}

describe('interpretKey', () => {
  it('maps j to move +1', () => {
    expect(interpretKey(input({ key: 'j' }))).toEqual({ type: 'move', delta: 1 })
  })

  it('maps k to move -1', () => {
    expect(interpretKey(input({ key: 'k' }))).toEqual({ type: 'move', delta: -1 })
  })

  it('maps a to verdict approve', () => {
    expect(interpretKey(input({ key: 'a' }))).toEqual({ type: 'verdict', verdict: 'approve' })
  })

  it('maps x to verdict request-changes', () => {
    expect(interpretKey(input({ key: 'x' }))).toEqual({
      type: 'verdict',
      verdict: 'request-changes',
    })
  })

  it('returns null for c (the removed comment-verdict binding)', () => {
    expect(interpretKey(input({ key: 'c' }))).toBeNull()
  })

  it('maps i to focus-comment', () => {
    expect(interpretKey(input({ key: 'i' }))).toEqual({ type: 'focus-comment' })
  })

  it('maps Enter to confirm-submit', () => {
    expect(interpretKey(input({ key: 'Enter' }))).toEqual({ type: 'confirm-submit' })
  })

  it('returns null for an unknown key', () => {
    expect(interpretKey(input({ key: 'q' }))).toBeNull()
  })

  it('returns null for Cmd+C (copy must not become a verdict)', () => {
    expect(interpretKey(input({ key: 'c', metaKey: true }))).toBeNull()
  })

  it('returns null for Ctrl+A (select-all must not become approve)', () => {
    expect(interpretKey(input({ key: 'a', ctrlKey: true }))).toBeNull()
  })

  it('returns null for Cmd+X', () => {
    expect(interpretKey(input({ key: 'x', metaKey: true }))).toBeNull()
  })

  it('returns null for Alt+x', () => {
    expect(interpretKey(input({ key: 'x', altKey: true }))).toBeNull()
  })

  it('returns null while an IME composition is in progress', () => {
    expect(interpretKey(input({ key: 'j', isComposing: true }))).toBeNull()
  })

  it('returns null for a binding key typed inside a TEXTAREA', () => {
    expect(interpretKey(input({ key: 'j', targetTag: 'TEXTAREA' }))).toBeNull()
  })

  it('returns null for a binding key typed inside an INPUT', () => {
    expect(interpretKey(input({ key: 'a', targetTag: 'INPUT' }))).toBeNull()
  })

  it('returns null for a binding key typed inside a SELECT', () => {
    expect(interpretKey(input({ key: 'Enter', targetTag: 'SELECT' }))).toBeNull()
  })

  it('returns null for a binding key typed inside a contenteditable element', () => {
    expect(interpretKey(input({ key: 'a', targetTag: 'DIV', targetEditable: true }))).toBeNull()
  })

  it('returns null for an unknown key even when not editable', () => {
    expect(interpretKey(input({ key: 'z' }))).toBeNull()
  })
})
