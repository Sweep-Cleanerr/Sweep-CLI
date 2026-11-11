# Sweep vs BleachBit CLI benchmark

Measures Sweep's preview path, optionally against BleachBit's command-line
preview on the same cleaner options, in one harness session. It is a **safe**
benchmark: the script only runs `preview` / `--preview`, never `clean`, so it
does not delete files.

Sweep-only is the default. Pass `-IncludeBleachBit` (or
`--include-bleachbit`) to run Sweep and BleachBit back-to-back for each
measured sample.

## Requirements

- Sweep built in release mode (or pass `-SweepPath`):

  ```powershell
  cargo build --release
  ```

- BleachBit installed when using `-IncludeBleachBit`. The script looks for
  `bleachbit_console.exe` in the usual Program Files locations and on `PATH`,
  or pass `-BleachBitPath`. Portable installs live outside those locations,
  so they need that flag.

- A config file Sweep can read. In a sandboxed session the user profile may
  be unreadable; pass `-SweepConfig` with a copy of the config inside the
  workspace. The flag is forwarded as `--config` to every Sweep invocation,
  which also makes the run independent of whatever the user changed later.

- PowerShell 5.1 or newer.

## Run

```powershell
# Compare against BleachBit
powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1 -IncludeBleachBit

# Sweep only
powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1
```

Useful variations:

```powershell
# More stable timings
powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1 -IncludeBleachBit -Runs 10 -Warmup 2

# Only a few cleaners
powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1 `
    -IncludeBleachBit -Selectors system.tmp,firefox.cache

# Explicit install paths (portable BleachBit + isolated config)
powershell -ExecutionPolicy Bypass -File .\bench\compare-bleachbit.ps1 `
    -IncludeBleachBit `
    -SweepPath .\target\release\sweep.exe `
    -SweepConfig .\bench\sweep-config.toml `
    -BleachBitPath 'C:\path\to\BleachBit-Portable\bleachbit_console.exe'
```

`-BleachBitPath` is required for portable installs when `-IncludeBleachBit` is
used. `-SweepConfig` is optional on an unrestricted machine but recommended
for reproducibility; the benchmark never writes to that config.

The script validates every selector against `sweep list --json` and
`bleachbit --list-cleaners` before running anything.

## Default pairs

Sweep and BleachBit do not share every cleaner ID. The default set is:

| Sweep selector | BleachBit selector | Note |
| --- | --- | --- |
| `system.tmp` | `system.tmp` | |
| `firefox.cache` | `firefox.cache` | |
| `firefox.cookies` | `firefox.cookies` | |
| `firefox.history` | `firefox.url_history` | Different option ID |
| `firefox.session` | `firefox.session` | |
| `browsers.chrome_cache` | `google_chrome.cache` | Sweep folds browser cleaners into one cleaner |
| `browsers.chrome_history` | `google_chrome.history` | |
| `browsers.chrome_cookies` | `google_chrome.cookies` | |
| `browsers.edge_cache` | `microsoft_edge.cache` | |
| `discord.cache` | `discord.cache` | |
| `slack.cache` | `slack.cache` | |

Use `-Selectors` to replace the list. A pair can map to multiple BleachBit
selectors with commas:

```powershell
-Selectors 'system.tmp','system.trash=system.recycle_bin'
```

## What is measured

For every pair and run:

- **Wall clock** from process start to exit, including interpreter/binary
  startup, definition loading, the preview scan, and output generation.
- Exit code, stdout/stderr size, output line count, and the first stderr
  line on failure.
- Sweep's own `duration_ms`, `files_removed`, and `reclaimed_bytes` from its
  compact JSON report.

Sweep runs as `sweep --json --summary preview <selector>` (with
`--config <path>` first when `-SweepConfig` is used); BleachBit runs as
`bleachbit --preview <selector>` when `-IncludeBleachBit` is passed. Warmup
runs are discarded, and measured runs alternate tool order to reduce
cache/order bias. Medians use successful runs only; failures are counted in
the report.

`reclaimed_bytes` is shown for context, not as a fairness score: BleachBit's
CLI has no machine-readable report with the same counters. The summary also
shows `Sweep files` next to `BB lines` (BleachBit preview output lines) so a
large semantic mismatch is visible instead of hidden inside the ratio. For
small scans, process startup is a large part of wall time, so use `-Runs 10`
or more.

The generated Markdown uses executable leaf names, not absolute paths, so it
contains no home-directory or username information.

## Output

Each run writes two files under `bench/results/`:

- `compare-bleachbit-<timestamp>.csv` — one row per sample.
- `compare-bleachbit-<timestamp>.md` — machine metadata and the summary table.

If the output directory becomes unwritable during a long run, both files are
written to `sweep-bench-<timestamp>` under the system TEMP directory instead
and a warning is printed; the console summary is always produced.

The console also prints the summary. `BB/Sweep` above `1.00x` means Sweep
was faster on that pair.

## Latest results (2026-11-11)

All 11 default selectors, 220 measured samples, 0 failures:
`results/compare-bleachbit-20261111-173150.csv` and
`results/compare-bleachbit-20261111-173150.md`. 10 measured runs + 2 warmups
per tool per pair, the 0.6 performance build (reported as Sweep 0.5.1 before
the version bump), BleachBit 6.0.6
(portable), 11th Gen Intel Core i5-1135G7, Windows 11 Pro 10.0.26200. Sweep
and BleachBit alternated back-to-back in the same session, using the isolated
`bench/sweep-config.toml`.

| Pair | Sweep (ms) | BB (ms) | BB/Sweep | Sweep files | BB lines |
| --- | ---: | ---: | ---: | ---: | ---: |
| system.tmp | 1,453.2 | 16,163.2 | 11.12x | 833 | 43,590 |
| firefox.cache | 425.5 | 480.6 | 1.13x | 0 | 2 |
| firefox.cookies | 418.3 | 434.5 | 1.04x | 0 | 2 |
| firefox.history | 438.5 | 487.7 | 1.11x | 0 | 2 |
| firefox.session | 365.2 | 385.1 | 1.05x | 0 | 2 |
| browsers.chrome_cache | 352.4 | 366.8 | 1.04x | 0 | 2 |
| browsers.chrome_history | 430.3 | 523.0 | 1.22x | 0 | 2 |
| browsers.chrome_cookies | 416.0 | 424.3 | 1.02x | 0 | 2 |
| browsers.edge_cache | 1,642.0 | 2,389.6 | 1.46x | 11,354 | 11,561 |
| discord.cache | 370.7 | 419.8 | 1.13x | 0 | 2 |
| slack.cache | 385.2 | 434.5 | 1.13x | 0 | 2 |

Affected selectors, before and after the 0.6 performance fixes:

| Pair | Sweep before (ms) | Sweep after (ms) | BB after (ms) | BB/Sweep | Sweep files | BB lines |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| browsers.edge_cache | 12,747.9 | 1,642.0 | 2,389.6 | 1.46x | 11,354 | 11,561 |
| browsers.chrome_cache | 1,122.9 | 352.4 | 366.8 | 1.04x | 0 | 2 |
| browsers.chrome_history | 1,559.1 | 430.3 | 523.0 | 1.22x | 0 | 2 |
| browsers.chrome_cookies | 1,273.5 | 416.0 | 424.3 | 1.02x | 0 | 2 |

"Before" medians are the pre-fix 10-run pass in
`results/compare-bleachbit-20261110-202204.csv` on the same machine and tool
versions; they are not a simultaneous A/B run.

How to read it:

- Only `system.tmp` and `browsers.edge_cache` had comparable volume. Edge is
  now 1.46x faster than BleachBit (11,354 files / 724.6 MiB vs 11,561 lines)
  and 7.8x faster than Sweep's own pre-fix 12,747.9 ms.
- The three `chrome_*` pairs still report 0 files, so they mostly measure
  discovery and process startup. Sweep is slightly faster than BleachBit on
  all three (1.02-1.22x).
- 9 of 11 default pairs find nothing on either side. A fixture-backed
  selector set is needed before any throughput claim can be made about those
  cleaners.
- `system.tmp` measured 11.12x in this pass; BleachBit ranged from 12.9 s to
  32.9 s. An earlier same-day four-selector pass measured 16.09x. Quote the
  pass you are citing, and quote medians rather than single runs.

The earlier 5-run pass (`...182001.*`) ran on a release binary built before a
change to cleaner variable resolution. That binary resolved browser profile
variables to nothing, so `edge_cache` reported 0 files and appeared 6.36x
faster. Those numbers are superseded and are not comparable.

## Limits

- This is a **preview** benchmark. It measures discovery and reporting, not
  deletion throughput. A clean benchmark would need a disposable VM or
  snapshot restored before each tool, because the first tool changes what
  the second one sees.
- Cleaner pairs are semantic approximations. Different path rules, lock
  checks, and keep-lists can make one side scan more files.
- On this machine 9 of the 11 default pairs have no matching data, so their
  timings are startup-dominated. Add fixture-backed pairs with `-Selectors`
  before making throughput claims.
- Run on an idle machine; Windows Defender, indexing, and open browsers add
  noise.
- If BleachBit asks for elevation, run the benchmark from an elevated
  terminal or pass `-BleachBitExtraArgs '--no-uac'`.
