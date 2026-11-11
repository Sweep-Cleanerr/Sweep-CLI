# Scripts and CI

## The contract

- One JSON document on stdout with `--json`; humans read counters.
- `--progress-json` adds newline-delimited progress objects on
  **stderr**, so the stdout document stays parseable.
- Exit `0` = clean success; `1` = error / abort / refusal.
- `clean` without `--yes` exits `1` and deletes nothing — that is
  the consent gate, not an inconvenience.

## Copy-paste job

```sh
#!/bin/sh
# Preview in CI, clean only on manual approval.
sweep --json preview --all > preview.json
test "$(jq .errors preview.json)" = "0" || exit 1
echo "review preview.json, then run:"
echo "  sweep --json clean --all --yes > clean.json"
```

```yaml
# .github/workflows/sweep.yml (sketch)
steps:
  - run: sweep preview --all
  - run: sweep clean --all --yes   # only on protected branches + approval
```

## Determinism notes

- `preview` and `clean --json` reports contain `duration_ms` —
  ignore it when diffing.
- Entry order follows engine merge order; compare by
  `by_option` / counters, not raw arrays, across parallel runs.
- With `--backup-dir`, `net_reclaimed_bytes` stays `0` until
  `backup prune` runs — do not fail a disk-space assertion on the
  pre-prune number.
