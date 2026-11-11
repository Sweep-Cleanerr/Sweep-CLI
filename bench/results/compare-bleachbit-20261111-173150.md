# Sweep vs BleachBit CLI benchmark

- Date: 2026-11-11 17:31:50
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
| system.tmp | system.tmp | 1,453.2 | 796.4-2,368.0 | 16,163.2 | 12,886.8-32,921.5 | 11.12x | 833 | 43,590 | 27.7 MiB | 0/0 |
| firefox.cache | firefox.cache | 425.5 | 388.7-546.4 | 480.6 | 427.9-536.9 | 1.13x | 0 | 2 | 0.0 B | 0/0 |
| firefox.cookies | firefox.cookies | 418.3 | 362.9-562.0 | 434.5 | 410.9-1,111.0 | 1.04x | 0 | 2 | 0.0 B | 0/0 |
| firefox.history | firefox.url_history | 438.5 | 362.2-561.1 | 487.7 | 396.0-659.7 | 1.11x | 0 | 2 | 0.0 B | 0/0 |
| firefox.session | firefox.session | 365.2 | 346.3-429.7 | 385.1 | 379.4-518.2 | 1.05x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_cache | google_chrome.cache | 352.4 | 340.6-629.2 | 366.8 | 348.0-408.7 | 1.04x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_history | google_chrome.history | 430.3 | 383.8-626.6 | 523.0 | 435.4-1,008.8 | 1.22x | 0 | 2 | 0.0 B | 0/0 |
| browsers.chrome_cookies | google_chrome.cookies | 416.0 | 379.8-567.8 | 424.3 | 398.9-730.7 | 1.02x | 0 | 2 | 0.0 B | 0/0 |
| browsers.edge_cache | microsoft_edge.cache | 1,642.0 | 1,584.6-2,249.2 | 2,389.6 | 2,048.8-2,939.8 | 1.46x | 11,354 | 11,561 | 724.6 MiB | 0/0 |
| discord.cache | discord.cache | 370.7 | 355.7-401.3 | 419.8 | 400.4-652.4 | 1.13x | 0 | 2 | 0.0 B | 0/0 |
| slack.cache | slack.cache | 385.2 | 360.8-424.5 | 434.5 | 417.1-567.9 | 1.13x | 0 | 2 | 0.0 B | 0/0 |

Raw samples: `compare-bleachbit-20261111-173150.csv`
