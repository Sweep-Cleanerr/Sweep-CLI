# Benchmarks

Sweep's preview path against the BleachBit CLI preview. Every run is
preview-only: no files are deleted. Method and limits are in
[`bench/README.md`](../bench/README.md).

## Sweep 0.5.1 vs BleachBit 6.0.6

One same-day pass over all 11 default pairs, with Sweep and BleachBit run
back-to-back in the same harness session:

- Date: 2026-11-11
- CPU: 11th Gen Intel(R) Core(TM) i5-1135G7 @ 2.40GHz
- OS: Microsoft Windows 11 Pro 10.0.26200, 31.7 GiB RAM
- Runs per tool: 10 measured + 2 warmups
- Sweep config: [`bench/sweep-config.toml`](../bench/sweep-config.toml)
- Raw samples: [`compare-bleachbit-20261111-173150.csv`](../bench/results/compare-bleachbit-20261111-173150.csv)
  and [`compare-bleachbit-20261111-173150.md`](../bench/results/compare-bleachbit-20261111-173150.md)

| Sweep selector | BleachBit selector | Sweep median (ms) | BleachBit median (ms) | BB/Sweep | Sweep files | BB lines |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| system.tmp | system.tmp | 1,453.2 | 16,163.2 | 11.12x | 833 | 43,590 |
| firefox.cache | firefox.cache | 425.5 | 480.6 | 1.13x | 0 | 2 |
| firefox.cookies | firefox.cookies | 418.3 | 434.5 | 1.04x | 0 | 2 |
| firefox.history | firefox.url_history | 438.5 | 487.7 | 1.11x | 0 | 2 |
| firefox.session | firefox.session | 365.2 | 385.1 | 1.05x | 0 | 2 |
| browsers.chrome_cache | google_chrome.cache | 352.4 | 366.8 | 1.04x | 0 | 2 |
| browsers.chrome_history | google_chrome.history | 430.3 | 523.0 | 1.22x | 0 | 2 |
| browsers.chrome_cookies | google_chrome.cookies | 416.0 | 424.3 | 1.02x | 0 | 2 |
| browsers.edge_cache | microsoft_edge.cache | 1,642.0 | 2,389.6 | 1.46x | 11,354 | 11,561 |
| discord.cache | discord.cache | 370.7 | 419.8 | 1.13x | 0 | 2 |
| slack.cache | slack.cache | 385.2 | 434.5 | 1.13x | 0 | 2 |

`BB/Sweep` above `1.00x` means Sweep was faster. Only `system.tmp` and
`browsers.edge_cache` did work on both sides on this machine. For the other
nine pairs, `BB lines = 2` is BleachBit's localized zero-result summary, so
those rows measure process startup and profile discovery, not cleaner
throughput.

### Affected selectors: before and after

| Sweep selector | Sweep before (ms) | Sweep after (ms) | BleachBit (ms) | BB/Sweep | Sweep files | BB lines |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| browsers.edge_cache | 12,747.9 | 1,642.0 | 2,389.6 | 1.46x | 11,354 | 11,561 |
| browsers.chrome_cache | 1,122.9 | 352.4 | 366.8 | 1.04x | 0 | 2 |
| browsers.chrome_history | 1,559.1 | 430.3 | 523.0 | 1.22x | 0 | 2 |
| browsers.chrome_cookies | 1,273.5 | 416.0 | 424.3 | 1.02x | 0 | 2 |

"Before" medians come from the pre-fix 10-run pass in
[`compare-bleachbit-20261110-202204.csv`](../bench/results/compare-bleachbit-20261110-202204.csv),
measured on the same machine and tool versions. They are not a simultaneous
A/B run. "After" values are from the all-selector pass above.

An earlier same-day 10-run pass on only these four selectors measured
`system.tmp` at 16.09x and Edge at 1.26x
([`compare-bleachbit-20261111-171004.csv`](../bench/results/compare-bleachbit-20261111-171004.csv)).
The all-selector pass is the final result; quote it unless discussing that
earlier pass explicitly.
