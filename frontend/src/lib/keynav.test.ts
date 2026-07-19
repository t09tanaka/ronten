import { describe, expect, it } from 'vitest'
import { interpretKey } from './keynav'

describe('interpretKey', () => {
  it('maps j to move +1', () => {
    expect(interpretKey('j', 'BODY', false)).toEqual({ type: 'move', delta: 1 })
  })

  it('maps k to move -1', () => {
    expect(interpretKey('k', 'BODY', false)).toEqual({ type: 'move', delta: -1 })
  })

  it('maps a to verdict approve', () => {
    expect(interpretKey('a', 'BODY', false)).toEqual({ type: 'verdict', verdict: 'approve' })
  })

  it('maps x to verdict request-changes', () => {
    expect(interpretKey('x', 'BODY', false)).toEqual({
      type: 'verdict',
      verdict: 'request-changes',
    })
  })

  it('maps c to verdict comment', () => {
    expect(interpretKey('c', 'BODY', false)).toEqual({ type: 'verdict', verdict: 'comment' })
  })

  it('maps i to focus-comment', () => {
    expect(interpretKey('i', 'BODY', false)).toEqual({ type: 'focus-comment' })
  })

  it('maps Enter to confirm-submit', () => {
    expect(interpretKey('Enter', 'BODY', false)).toEqual({ type: 'confirm-submit' })
  })

  it('returns null for an unknown key', () => {
    expect(interpretKey('q', 'BODY', false)).toBeNull()
  })

  it('returns null for a binding key typed inside a TEXTAREA', () => {
    expect(interpretKey('j', 'TEXTAREA', false)).toBeNull()
  })

  it('returns null for a binding key typed inside an INPUT', () => {
    expect(interpretKey('a', 'INPUT', false)).toBeNull()
  })

  it('returns null for a binding key typed inside a SELECT', () => {
    expect(interpretKey('Enter', 'SELECT', false)).toBeNull()
  })

  it('returns null for a binding key typed inside a contenteditable element', () => {
    expect(interpretKey('a', 'DIV', true)).toBeNull()
  })

  it('returns null for an unknown key even when not editable', () => {
    expect(interpretKey('z', 'BODY', false)).toBeNull()
  })
})
