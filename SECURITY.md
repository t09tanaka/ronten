# Security Policy

## Supported versions

Only the latest released version receives security fixes. ronten is
pre-1.0; there are no maintenance branches.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting on this repository
("Security" tab → "Report a vulnerability"). Do not open a public issue for
an unfixed vulnerability. You can expect an initial response within a week.

## Threat model (summary)

ronten's job is to show a human an *honest* rendering of a committed git
diff and record their review of it. The design assumes a
**trusted-but-fallible launching agent**:

- The diff is reconstructed from blob objects via git plumbing with
  replace-refs/grafts disabled and repo-redirecting environment variables
  scrubbed, so an in-repo agent cannot cosmetically alter what the reviewer
  sees (`.gitattributes`, textconv, diff drivers, fake `HEAD`).
- Results are pinned to the reviewed commits (`review.base_oid` /
  `head_oid` / `merge_base_oid` plus content digests), and submit re-checks
  `HEAD`, so a result cannot be silently re-applied to a different commit.
- The server binds `127.0.0.1` only, requires a random per-session URL
  token, sets strict security headers, and accepts exactly one outcome per
  process.

**Out of scope / known limitation**: the launching agent reads the session
URL from stderr and therefore knows the token. ronten **cannot prove the
submit came from a human** — every result carries
`"assurance": "advisory"` and must not be used as a security-enforcing
approval gate (e.g. a CI branch-protection check). This is documented in
the README's trust-boundary section; a cryptographic "secure gate" mode
(out-of-band token delivery, signed results) is a possible future
architecture, not a property of the current design.
