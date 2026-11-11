# Usage guide

Everything below is the one-shot contract: `sweep <command> …`.
There is no interactive shell. `sweep` with no arguments prints
help; `sweep <command> --help` documents each command.

Examples use a disposable fixture. Point `--define DIR` at it, never
at a real home directory:

```sh
sweep --define ./defs preview demo.cache
```

## Pages, in reading order

| Page | Answers |
| --- | --- |
| [01 · The working loop](01-basic-workflow.md) | preview → clean → undo/purge, the `--yes` gate, `--backup-dir` vs `--trash-dir`. |
| [02 · Choosing what to clean](02-selecting.md) | selectors, `--exclude`, `--os`, `--define`, `list` / `info` / `import`. |
| [03 · Reading the output](03-reading-output.md) | human counters, `--json` / `--summary` shapes, exit codes. |
| [04 · Scripts and CI](04-automation.md) | the stdout contract, JSON keys, `--yes`, a copy-paste job. |
| [05 · When something refuses](05-troubleshooting.md) | refusal messages verbatim, what each one means, what to change. |
| [06 · No shell (by design)](06-no-shell.md) | why there is no interactive shell/TUI. |

## The three things worth knowing before anything else

1. **Nothing deletes until you say so twice.** `preview` never
   deletes. `clean` refuses without `--yes` and exits `1`.
2. **Undo is a ledger, not a promise.** `undo` only restores what
   `--backup-dir` copied or `--trash-dir` moved. A plain `clean`
   keeps nothing.
3. **The report is the contract.** `--json` prints one document to
   stdout; exit `0` means no errors and no abort, `1` otherwise.
