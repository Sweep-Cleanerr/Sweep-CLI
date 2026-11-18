# Changelog

## v0.6.2

Security fixes for guard/path bypasses:

- Root-level links: when a path passes through a junction or symlink directly
  under a filesystem root, Guard now canonicalizes and verifies the resolved
  target, so protected subtrees cannot be hidden behind the link.
- File-action links: a leaf link inside a protected parent is no longer
  accepted just because its resolved target is safe — the parent is checked too.
- Link cache: negative link probes are no longer cached, so a directory replaced
  with a junction inside the 2 s TTL cannot bypass both safety gates.
- Windows path comparison: trailing `.` and spaces in components are ignored
  when matching keep/protected paths, matching Win32 path normalization.

Verification: `cargo +1.88-x86_64-pc-windows-gnu test --all-targets` — 245 passed.
