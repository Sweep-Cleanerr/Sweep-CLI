# Sweep documentation

Use Sweep from the [README](../README.md). There is no interactive shell
or TUI by design: every command is one-shot, scriptable, and prints a
single JSON document with `--json`.

| Page | About |
| --- | --- |
| [overview.md](overview.md) | Project overview, architecture, and safety model. |
| [usage/README.md](usage/README.md) | Usage guide index. |
| [usage/01-basic-workflow.md](usage/01-basic-workflow.md) | preview → clean → undo/purge, the `--yes` gate, backup vs trash. |
| [usage/02-selecting.md](usage/02-selecting.md) | selectors, exclusions, platform filters, `--define`, `list` / `info` / `import`. |
| [usage/03-reading-output.md](usage/03-reading-output.md) | human output, `--json` / `--summary` shapes, exit codes. |
| [usage/04-automation.md](usage/04-automation.md) | stdout contract, `--json` keys, `--yes`, a copy-paste CI job. |
| [usage/05-troubleshooting.md](usage/05-troubleshooting.md) | refusal messages verbatim, what each one means, what to change. |
| [usage/06-no-shell.md](usage/06-no-shell.md) | Why there is no interactive shell/TUI. |
| [BENCHMARKS.md](BENCHMARKS.md) | measured Sweep vs BleachBit preview results. |
| [cleaner-format.md](cleaner-format.md) | the native TOML schema (authoritative for `src/definition/native.rs`). |
| [compatibility.md](compatibility.md) | provider coverage, OS gates, what is not yet runtime-tested. |

Rules Sweep follows:

1. Nothing deletes until you say so twice. `preview` never deletes.
   `clean` and every destructive command refuse without `--yes`.
2. Undo is a ledger, not a promise. `undo` can only restore what
   `--backup-dir` copied or `--trash-dir` moved. A plain `clean`
   deletes and keeps nothing.
3. The report is the contract. Plain text for people, `--json` for
   machines, one document on stdout, exit `0` on success and `1`
   when anything failed or was aborted.
