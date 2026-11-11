# No shell (by design)

Sweep is a one-shot CLI. It deliberately does **not** include an
interactive shell or a full-screen TUI.

- `sweep` alone prints help.
- `interactive` reports
  `interactive is not implemented; use the one-shot CLI`.
- There is no raw-mode terminal loop, no key handling, no screens, and
  no selections persisted in a shell.

## Why

A one-shot interface keeps the engine testable and scriptable: every
command runs without a terminal, and every command can be composed in a
shell or CI job.

The human-readable output (`Files removed: …`, counters, stderr
failures) and the `--json` report are the full contract. Any future UI
should sit *on top of* `engine::worker`, not beside a second engine.
