# Sweep

Sweep is a fast, scriptable system cleaner for Windows, Linux, and macOS.
It ships with built-in definitions for browser caches, temporary files,
logs, package-manager caches, and developer artifacts, and can also load
BleachBit XML, `winapp2.ini`, and drop-in TOML definitions.

Sweep is preview-first. `preview` never changes the filesystem, and every
destructive command requires an explicit `--yes`.

## Why Sweep

- **Fast traversal.** The preview path avoids redundant filesystem probes,
  bounds profile globs, and caches Git state across a run.
- **Safe by default.** Protected system roots, user-data folders, symlinks,
  junctions, and dirty Git worktrees are checked before deletion.
- **Scriptable.** Every command has a stable human-readable form and a
  one-document JSON form. `--progress-json` keeps progress on stderr.
- **Portable definitions.** Native TOML cleaners live in `cleaners/`;
  external BleachBit and `winapp2.ini` definitions are supported.
- **No shell required.** Sweep is a one-shot CLI by design; there is no
  interactive TUI.

## Build from source

Sweep requires Rust 1.88 or newer.

```sh
cargo build --release
./target/release/sweep --help
```

Install it on `PATH`:

```sh
cargo install --path .
```

## Quick start

```sh
# List available cleaners
sweep list

# Preview a selection; this never deletes anything
sweep preview system.tmp

# Preview every default selector as JSON
sweep --json --summary preview --all

# Clean only after reviewing the preview
sweep clean --yes system.tmp
```

Selectors use the form `cleaner.option`, for example
`browsers.edge_cache` or `system.tmp`. Use `sweep info <cleaner>` to see
the options and paths a cleaner would touch.

## Safety model

- `preview` is always a dry run. `clean` and other destructive commands
  refuse to run without `--yes`.
- Two independent guards run before a deletion: `core::keep::Guard`
  (protected roots, user data, keep-list, links and link ancestors) and
  `action::safety` (relative paths, critical trees, parent permissions).
- Symlinks and Windows junctions are never followed as delete targets.
  The link itself may be removed; its target is not touched.
- Git worktrees are protected by `--git-guard` (`off`, `warn`, `block`,
  `paranoid`). Untracked files do not make a repository dirty.
- Cleaner definitions from outside the built-in set are untrusted by
  default: privileged actions such as `process`, `winreg`, and
  `apt.autoremove` are stripped unless `--trust-external` is passed.
- `--backup-dir` and `--trash-dir` are opt-in. A plain `clean` keeps no
  copy; `undo` can only restore what one of those options saved.

## Benchmark

Sweep 0.5.1 was measured against BleachBit 6.0.6 on Windows 11 with 10
runs plus 2 warmups per pair. Preview-only, no files deleted:

| Selector | Sweep | BleachBit | Speed-up |
| --- | ---: | ---: | ---: |
| `system.tmp` | 1,453 ms | 16,163 ms | 11.12x |
| `browsers.edge_cache` | 1,642 ms | 2,390 ms | 1.46x |

Only those two pairs did real work on the test machine. The full
11-selector table, hardware details, raw CSV, and limitations are in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). The harness is
[`bench/compare-bleachbit.ps1`](bench/compare-bleachbit.ps1), and
[`bench/README.md`](bench/README.md) documents the method.

## Documentation

- [Overview and architecture](docs/overview.md)
- [Usage guide](docs/usage/README.md)
- [Cleaner definition format](docs/cleaner-format.md)
- [Provider compatibility](docs/compatibility.md)
- [Benchmarks](docs/BENCHMARKS.md)

## Platform support

| Platform | Status |
| --- | --- |
| Windows 10/11 x86_64 | Primary target; tested |
| Linux | Supported; runtime coverage is partial |
| macOS | Supported; runtime coverage is partial |

The CLI, cleaner resolution, file traversal, SQLite cleaners, and safety
guards are shared across platforms. Platform-specific maintenance commands
are listed in [compatibility.md](docs/compatibility.md).

## Development

```sh
cargo test --all-targets
cargo build --release
```

The test suite covers cleaner parsing, path expansion, traversal bounds,
safety refusals, Git guard invalidation, preview/clean contracts, and CLI
parsing.

## License

Sweep is licensed under the GNU General Public License, version 3 or
later. See [LICENSE](LICENSE).
