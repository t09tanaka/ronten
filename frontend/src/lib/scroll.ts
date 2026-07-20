import { tick } from 'svelte'

/// Bring the general-comments box into view and focus it — used when a
/// comment or request-changes verdict is chosen, mirroring the `i`
/// shortcut, so the next action (writing the reason) needs no extra
/// navigation. Instant scroll on purpose: animated scrolling stalls in
/// throttled or embedded browser contexts, and an instant jump also
/// satisfies prefers-reduced-motion without a media query.
export function focusGeneralComments(): void {
  // Deferred via tick() so this runs after the verdict change's re-render.
  // Not requestAnimationFrame: rAF never fires while a tab reports itself
  // hidden (background tabs, embedded webviews), which would silently
  // drop the scroll; tick() is microtask-based and always runs.
  void tick().then(() => {
    const el = document.getElementById('general-comment-textarea')
    if (!el) return
    el.scrollIntoView({ behavior: 'auto', block: 'center' })
    el.focus({ preventScroll: true })
  })
}
