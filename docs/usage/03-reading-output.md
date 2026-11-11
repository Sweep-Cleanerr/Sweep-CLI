# Reading the output

## Human output

```text
Files removed: 2
Reclaimed: 36B
Special operations: 0
Skipped: 0
Errors: 0
Blocked: 0
Warnings: 0
Duration: 2 ms
```

Failures, blocked items, and warnings follow on stderr
(`error: cleaner (option): message`). Operation-log mode lines
(`shred`, `trim`, `vacuum`, `ostrash`) are entries with
`EntryKind::Command` / `Vacuum` / `WipeFreeSpace`, not extra
counters — read them via `--json`.

`--quiet` suppresses the counter block. Registry load warnings go to
stderr unless quiet.

## JSON output

`--json` prints **one** JSON document to stdout. Full form
(`Report::to_json`, `schema_version: 1`):

```json
{
  "schema_version": 1,
  "reclaimed_bytes": 36,
  "files_removed": 2,
  "special_operations": 0,
  "skipped": 0,
  "errors": 0,
  "blocked": 0,
  "warnings": 0,
  "backed_up_bytes": 0,
  "net_reclaimed_bytes": 36,
  "aborted": false,
  "duration_ms": 2,
  "bytes_affected": 36,
  "items_count": 2,
  "entries": [ /* per-file entries, path serialized lossily */ ],
  "failures": []
}
```

`--summary` adds `entries_truncated`, `by_option` (cleaner.option →
files/bytes), and samples entries across the whole run (first/last
included). `by_option` is sorted; counters stay exact.

Counts: `files_removed` counts `Delete` + `Shred` entries only;
`special_operations` counts `CleanFile | Vacuum | Registry |
Command | WipeFreeSpace`; `reclaimed_bytes` sums every entry's
reclaimed bytes but is overridden when the engine supplies merged
totals. `blocked` counts refusals (protected path, running app,
destructive gate), which are **not** errors.

## Exit codes

- `0` — report printed, no errors, not aborted.
- `1` — any error, or `report.aborted` (Ctrl-C / cancel flag), or
  a usage refusal (`clean` without `--yes`, unknown selector,
  unsupported command).

There is no exit code `2` or `3`.
