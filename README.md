<div align="center">

# Sweep

**Fast, preview-first system cleaning for Windows, Linux, and macOS.**

[![Release](https://img.shields.io/github/v/release/Sweep-Cleanerr/Sweep-CLI?label=release)](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases)
[![Version](https://img.shields.io/badge/version-0.6.1-blue.svg)](Cargo.toml)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey.svg)](#platform-support)
[![Tests](https://img.shields.io/badge/tests-240%20passing-brightgreen.svg)](#development)

[Quick start](#quick-start) · [Benchmarks](#benchmarks) · [Safety](#safety-model) · [Documentation](#documentation) · [Releases](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases)

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

### Windows binaries

Portable, single-file builds are published on the
[v0.6.1 release](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/tag/v0.6.1).
There is no installer: download, unzip if needed, and run `sweep.exe`.

| Architecture | Build | Download |
| --- | --- | --- |
| x64 | MSVC (recommended) | [zip](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-x86_64-pc-windows-msvc-portable.zip) · [exe](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-x86_64-pc-windows-msvc.exe) |
| x64 | GNU | [zip](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-x86_64-pc-windows-gnu-portable.zip) · [exe](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-x86_64-pc-windows-gnu.exe) |
| ARM64 | MSVC | [zip](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-aarch64-pc-windows-msvc-portable.zip) · [exe](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/download/v0.6.1/sweep-v0.6.1-aarch64-pc-windows-msvc.exe) |

The builds are unsigned; Windows SmartScreen may ask for confirmation on first
run. Verify the download with the SHA-256 checksums below if needed.

```text
sweep-v0.6.1-x86_64-pc-windows-msvc-portable.zip   4c1b8c36448a3b7248ebba50db29e35e48ff7dc68929cf67f6a711ce7f571489
sweep-v0.6.1-x86_64-pc-windows-msvc.exe            857a3a608dec05f2db69a1520ac682354ed78d1785f6b95680ff267447717a85
sweep-v0.6.1-x86_64-pc-windows-gnu-portable.zip    4225854563b4f375ba8b681d94b959d342d8ab216254a07973e0898c7d363a63
sweep-v0.6.1-x86_64-pc-windows-gnu.exe             bb25feb2eba26183a22bd2c385282f5ed369ccfcb3f1e7fe0181f91196886892
sweep-v0.6.1-aarch64-pc-windows-msvc-portable.zip  83d34ba799fd20ab595c022590d87581a32236562a50485972e86066b332e878
sweep-v0.6.1-aarch64-pc-windows-msvc.exe           97a92bcb4b0cfab6d8989b154fc31b405dc594f4a31b8edc58b9936c2ca4d2f8
```

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

## Releases

Release notes and source archives are published on
[GitHub Releases](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases).
The current release is [v0.6.1](https://github.com/Sweep-Cleanerr/Sweep-CLI/releases/tag/v0.6.1).

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
