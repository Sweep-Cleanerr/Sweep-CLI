# When something refuses

## `refusing to clean without --yes`

`clean` (and destructive scan commands) need `--yes`. Without it:
exit `1`, nothing deleted. Add `--yes` explicitly; don't script
around it by aliasing.

## `cleaner not found` / `option not found` / `nothing selected`

Selector typo or the option is OS-gated off. `sweep list <id>` and
`sweep info <id>` show what exists; `sweep list --os <target>` shows
what the target host would run.

## `blocked: … (use --force to override)`

A cleaner whose `running` check matches (currently: a `running.path`
that exists). The worker blocks it unless `--force`. `--force`
does **not** lift protected roots or the keep-list.

## `refusing to operate on …` (protected path / link / relative)

Guard or safety refusal. The path is a protected root, user-data
leaf, critical tree, or a symlink/junction chain. There is no flag
to override it — fix the definition or the keep-list.

## `deep cleaner 'X' …` / tool errors

Deep cleaners run through `deep/cleaners.rs`; they call external
tools (`docker`, `journalctl`, `apt`, …). A missing tool or a
refused command is reported as a warning/failure for that option
instead of silently succeeding. `preview` and `list` still work
without the tool installed.

## `interactive is not implemented`

Sweep has no interactive shell or TUI by design. Use the one-shot
commands (`preview`, `clean`, `list`, …) instead. Every other command is
wired; maintenance helpers may report a "would do" entry instead of
changing the system.

## Registry warnings on stderr

`invalid cleaner definition in …` / `unknown action command …` —
the definition is skipped, the run continues. Syntax-check with
`sweep import <file>` instead of guessing.
