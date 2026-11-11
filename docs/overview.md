# Overview

## What Sweep is

Sweep is a one-shot system cleaner. It resolves a selector such as
`system.tmp` or `browsers.edge_cache` to cleaner definitions, walks the
declared paths, applies filters and safety gates, and reports what it
would remove. `clean` performs the deletions after an explicit `--yes`.

The binary and library share the same engine. Every command is
scriptable, and `--json` emits one report document on stdout.

Status: **240 tests passing** (`cargo test --all-targets`).

## Safety model

- Two gates on every deletion: `core::keep::Guard` (keep-list,
  protected roots, user data, symlink/ancestor checks) and
  `action::safety` (no relative paths, no critical trees, explicit
  allow-list, parent-writable checks).
- `preview` and `--preview` never mutate the filesystem. Machine-wide
  actions (clipboard, DNS, trim, …) report a "would run" entry.
- Destructive commands (`clean`, `deepscan` without `--preview`,
  `bigfiles --clean`, `dupes --clean`, `devscan --clean`,
  `leftovers --clean`, `purge`, `undo`, `shred`) refuse without
  `--yes`. Exit `1`, nothing touched.
- `shred`, `--backup-dir`, `--trash-dir` only act when explicitly
  configured (CLI flags or `config.toml`). The operation log is
  appended per entry (`engine/logline`).
- Symlinks and Windows junctions are never followed as delete
  targets: the link itself is unlinked, the target is never touched.
- External definitions lose their privileged actions (`process`,
  `winreg`, `apt.autoremove`) unless `--trust-external` is passed.
  Unknown commands fail closed in validation.

## Architecture

```
main → cli → engine/worker → action/{file,sqlite,structured,system}
                         → deep/cleaners
                         → fsutil/{walk,size,delete,run,freespace,reflink}
                         → shred/overwrite
core: error, path, keep (Guard), gitguard, report (+ HTML/Markdown)
definition: model, native (TOML), cleanerml, winapp2, mod (registry)
platform: dirs, device/space, windows/unix/linux helpers, winreg
config: settings, load/save
```

`cli/mod.rs` exposes the full command surface on the engine. The
`interactive` command is intentionally unsupported: Sweep has no
interactive shell or TUI. Some maintenance commands are lightweight
and report what they would do instead of changing the system. Deep
cleaners run through `deep/cleaners.rs` with the same Guard + safety
gates.

## Deliberate limits

- Interactive shell and full-screen TUI: not provided.
- Translation packs: the language setting is validated and stored,
  but only English strings ship.
- Several maintenance helpers (scheduler, process-name detection,
  OS recycle-bin deletion, persistent index) are best-effort and not
  exercised against every target platform.
