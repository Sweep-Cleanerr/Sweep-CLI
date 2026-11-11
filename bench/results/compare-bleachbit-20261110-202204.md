# Sweep vs BleachBit CLI benchmark

- Date: 2026-11-10 20:22:04
- Sweep: `sweep 0.5.1` (`sweep.exe`)
- BleachBit: `BleachBit version 6.0.6` (`bleachbit_console.exe`)
- CPU: 11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz
- OS: Microsoft Windows 11 Pro 10.0.26200
- RAM: 31.7 GiB
- PowerShell: 5.1.26100.9711
- Runs per tool: 10 (warmup: 2)
- Commands: `sweep --config <isolated config> --json --summary preview <selector>` vs `bleachbit --preview <selector>`
- Wall clock includes process startup and output generation. Medians use successful runs only.
- `BB/Sweep` above 1.00x means Sweep was faster. Byte counts are Sweep-only context.
- `BB lines` is BleachBit preview output lines; compare it with `Sweep files` to spot semantic mismatches.
- Preview only: no files were deleted.

| Sweep selector | BleachBit selector(s) | Sweep median (ms) | Sweep min-max | BleachBit median (ms) | BleachBit min-max | BB/Sweep | Sweep files | BB lines | Sweep bytes | Failures (S/B) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| system.tmp | system.tmp | 771.6 | 732.6-2,028.4 | 12,413.0 | 9,225.8-21,192.4 | 16.09x | 860 | 47,858 | 24.0 MiB | 0/0 |
| firefox.cache | firefox.cache | 505.2 | 419.8-1,122.8 | 592.2 | 501.6-712.3 | 1.17x | 0 | 2 | 0.0 B | 0/0 |
| firefox.cookies | firefox.cookies | 468.5 | 415.2-791.6 | 563.4 | 493.1-1,294.5 | 1.20x | 0 | 2 | 0.0 B | 0/0 |
| firefox.history | firefox.url_history | 474.0 | 405.9-571.7 | 515.4 | 476.1-828.5 | 1.09x | 0 | 2 | 0.0 B | 0/0 |
| firefox.session | firefox.session | 440.8 | 423.3-569.6 | 538.2 | 481.5-639.3 | 1.22x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_cache | google_chrome.cache | 1,122.9 | 1,064.2-1,231.0 | 550.3 | 502.2-747.7 | 0.49x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_history | google_chrome.history | 1,559.1 | 1,086.9-1,984.7 | 687.1 | 520.5-783.6 | 0.44x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_cookies | google_chrome.cookies | 1,273.5 | 1,102.9-1,812.2 | 615.5 | 578.1-706.4 | 0.48x | 0 | 2 | 0.0 B | 0/0 |
| browsers.edge_cache | microsoft_edge.cache | 12,747.9 | 11,016.0-15,703.0 | 1,891.4 | 1,589.3-3,980.0 | 0.15x | 11,354 | 11,561 | 724.6 MiB | 0/0 |
| discord.cache | discord.cache | 402.6 | 358.5-443.1 | 499.4 | 438.2-529.2 | 1.24x | 0 | 2 | 0.0 B | 0/0 |
| slack.cache | slack.cache | 397.9 | 368.2-424.2 | 499.3 | 433.4-594.6 | 1.25x | 0 | 2 | 0.0 B | 0/0 |

Raw samples: `compare-bleachbit-20261110-202204.csv`
