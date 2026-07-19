/// Bring the general-comments box into view and focus it — used when a
/// "comment" verdict is chosen, mirroring the `i` shortcut, so the next
/// action (writing the comment) needs no extra navigation. Instant scroll
/// on purpose: animated scrolling stalls in throttled or embedded browser
/// contexts, and an instant jump also satisfies prefers-reduced-motion
/// without a media query.
export function focusGeneralComments(): void {
  // Deferred one frame so this runs after the verdict change's re-render
  // settles the layout.
  requestAnimationFrame(() => {
    const el = document.getElementById('general-comment-textarea')
    if (!el) return
    el.scrollIntoView({ behavior: 'auto', block: 'center' })
    el.focus({ preventScroll: true })
  })
}
