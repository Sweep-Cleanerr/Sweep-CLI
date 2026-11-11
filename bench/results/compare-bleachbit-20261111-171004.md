# Sweep vs BleachBit CLI benchmark

- Date: 2026-11-11 17:10:04
- Sweep: `sweep 0.5.1` (`sweep.exe`)
- BleachBit: `BleachBit version 6.0.6` (`bleachbit_console.exe`)
- CPU: 11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz
- OS: Microsoft Windows 11 Pro 10.0.26200
- RAM: 31.7 GiB
- PowerShell: 5.1.26100.9711
- Runs per tool: 10 (warmup: 2)
- Commands: `sweep --config sweep-config.toml --json --summary preview <selector>` vs `bleachbit --preview <selector>`
- Wall clock includes process startup and output generation. Medians use successful runs only.
- `BB/Sweep` above 1.00x means Sweep was faster. Byte counts are Sweep-only context.
- `BB lines` is BleachBit preview output lines; compare it with `Sweep files` to spot semantic mismatches.
- Preview only: no files were deleted.

| Sweep selector | BleachBit selector(s) | Sweep median (ms) | Sweep min-max | BleachBit median (ms) | BleachBit min-max | BB/Sweep | Sweep files | BB lines | Sweep bytes | Failures (S/B) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| browsers.edge_cache | microsoft_edge.cache | 2,187.9 | 1,977.5-2,425.4 | 2,765.2 | 2,496.0-2,911.6 | 1.26x | 11,354 | 11,561 | 724.6 MiB | 0/0 |
| browsers.chrome_cache | google_chrome.cache | 472.5 | 437.1-909.0 | 499.0 | 410.9-740.5 | 1.06x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_history | google_chrome.history | 430.1 | 393.3-644.6 | 470.6 | 410.2-794.8 | 1.09x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_cookies | google_chrome.cookies | 562.8 | 395.4-808.9 | 653.3 | 492.8-785.7 | 1.16x | 0 | 2 | 0.0 B | 0/0 |

Raw samples: `compare-bleachbit-20261111-171004.csv`
