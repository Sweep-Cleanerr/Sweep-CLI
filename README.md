<div align="center">

# Sweep

**Fast, preview-first system cleaning for Windows, Linux, and macOS.**

[![Version](https://img.shields.io/badge/version-0.6.0-blue.svg)](Cargo.toml)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey.svg)](#platform-support)
[![Tests](https://img.shields.io/badge/tests-240%20passing-brightgreen.svg)](#development)

[Quick start](#quick-start) · [Benchmarks](#benchmarks) · [Safety](#safety-model) · [Documentation](#documentation)

</div>

Sweep is a one-shot CLI system cleaner. It removes disposable data such as
caches, temporary files, logs, package-manager artifacts, and developer build
output using 200+ built-in definitions. It can also load BleachBit XML,
`winapp2.ini`, and drop-in TOML cleaners.

The important part is what Sweep does *before* deleting: it resolves every
target, applies filters, runs two independent safety gates, and shows the
result. `preview` never writes to disk. Destructive commands require an
explicit `--yes`.

## Why Sweep

- **Preview-first by design.** Every run starts as a dry run. Nothing is
  removed until you pass `--yes` to a destructive command.
- **Fast where it matters.** The preview path avoids redundant filesystem
  probes, bounds browser-profile globs, caches Git state, and skips work when
  a selector resolves to nothing.
- **Safe by default.** Protected system roots, user-data folders, symlinks,
  junctions, and dirty Git worktrees are checked before deletion.
- **Scriptable.** Stable human-readable output plus a one-document JSON report.
  `--progress-json` keeps progress on stderr, so stdout stays parseable.
- **Portable definitions.** Native TOML cleaners live in `cleaners/`; external
  BleachBit and `winapp2.ini` definitions are supported.
- **No shell required.** Sweep is a one-shot CLI by design. There is no
  interactive TUI to maintain or secure.

## Quick start

Install from source:

```sh
cargo install --git https://github.com/Sweep-Cleanerr/Sweep-CLI
```

Or build locally:

```sh
cargo build --release
./target/release/sweep --help
```

On Windows the binary is `.\target\release\sweep.exe`.

```sh
# List available cleaners
sweep list

# Inspect a cleaner and its options
sweep info browsers

# Preview a selection; this never deletes anything
sweep preview browsers.edge_cache

# Preview every default selector as JSON
sweep --json --summary preview --all

# Clean only after reviewing the preview
sweep clean --yes system.tmp
```

Selectors use the form `cleaner.option`, for example `system.tmp`,
`browsers.edge_cache`, or `dev.npm_cache`. Run `sweep list` for the full set.

## What Sweep cleans

| Area | Examples |
| --- | --- |
| System | Temporary files, logs, thumbnails, crash dumps, recycle bin |
| Browsers | Chrome, Edge, Firefox, Brave, Vivaldi, Opera, Chromium |
| Developer | npm, pnpm, Yarn, Cargo, pip, Gradle, Maven, Go, Docker, VS Code |
| Applications | Discord, Slack, Teams, Spotify, Zoom, VLC, LibreOffice |

Definitions are plain TOML and live in [`cleaners/`](cleaners). To add a
cleaner, copy the closest definition, change the paths and filters, then load
it with `--define DIR` and inspect it with `sweep info <cleaner>`.

## Benchmarks

The 0.6 performance build was measured against BleachBit 6.0.6 on Windows 11
with 10 runs plus 2 warmups per pair. The benchmark binary reported `0.5.1`
because it was built before the version bump. Preview-only; no files were
deleted.

| Selector | Sweep | BleachBit | Speed-up |
| --- | ---: | ---: | ---: |
| `system.tmp` | 1,453 ms | 16,163 ms | **11.12x** |
| `browsers.edge_cache` | 1,642 ms | 2,390 ms | **1.46x** |

These two pairs did real work on the test machine. The other nine selectors
found zero files, so their timings measure startup and discovery only, not
cleaning work. Hardware, the full 11-selector table, raw CSV, and limitations
are in [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). The harness is
[`bench/compare-bleachbit.ps1`](bench/compare-bleachbit.ps1).

## Safety model

| Layer | What it does |
| --- | --- |
| Preview-first | `preview` is always a dry run; destructive commands refuse without `--yes`. |
| Path guards | `core::keep::Guard` and `action::safety` block protected roots, user data, relative paths, and critical trees. |
| Links | Symlinks and Windows junctions are never followed as delete targets. The link may be removed; its target is not touched. |
| Git guard | `--git-guard` (`off`, `warn`, `block`, `paranoid`) protects dirty worktrees. Untracked files do not make a repository dirty. |
| External definitions | Privileged actions such as `process`, `winreg`, and `apt.autoremove` are stripped unless `--trust-external` is passed. |
| Recovery | `--backup-dir` and `--trash-dir` are opt-in. A plain `clean` keeps no copy; `undo` restores only what those options saved. |

The full contract is documented in [`docs/overview.md`](docs/overview.md).

## Automation

Sweep keeps machine-readable output separate from progress:

```sh
# One JSON report on stdout
sweep --json --summary preview --all > preview.json

# Progress events on stderr while JSON stays clean
sweep --json --progress-json --summary preview --all > preview.json
```

Destructive commands still require `--yes`. Unknown selectors, usage errors,
and aborted runs exit with status `1`; success returns `0`.

## Platform support

| Platform | Status |
| --- | --- |
| Windows 10/11 x86_64 | Primary target; tested |
| Linux | Supported; runtime coverage is partial |
| macOS | Supported; runtime coverage is partial |

The CLI, cleaner resolution, file traversal, SQLite cleaners, and safety
guards are shared across platforms. Platform-specific maintenance commands are
listed in [`docs/compatibility.md`](docs/compatibility.md).

## Project layout

```text
src/         Engine, CLI, actions, safety, and platform helpers
cleaners/    Built-in TOML cleaner definitions
docs/        Usage, format, compatibility, and benchmark documentation
bench/       Reproducible Sweep vs BleachBit benchmark harness
```

## Documentation

| Document | Contents |
| --- | --- |
| [Overview and architecture](docs/overview.md) | How the engine and safety model fit together |
| [Usage guide](docs/usage/README.md) | Selecting, output, automation, troubleshooting |
| [Cleaner format](docs/cleaner-format.md) | TOML schema, variables, filters, trust rules |
| [Compatibility](docs/compatibility.md) | Provider families and platform coverage |
| [Benchmarks](docs/BENCHMARKS.md) | Method, hardware, full table, raw results |

## Development

```sh
cargo test --all-targets
cargo build --release
```

The suite covers cleaner parsing, path expansion, traversal bounds, safety
refusals, Git guard invalidation, preview/clean contracts, and CLI parsing.
Contributions are welcome through issues and pull requests.

## License

Sweep is licensed under the GNU General Public License, version 3 or later.
See [LICENSE](LICENSE).
