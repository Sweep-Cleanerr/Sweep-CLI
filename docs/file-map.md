# Contributor file map

For architecture and the safety model see [overview.md](overview.md).
This page answers a different question: **which file do I open to change X?**

## Root

| Path | Role |
| --- | --- |
| `src/` | All Rust code; `lib.rs` declares modules, `main.rs` calls `cli::main`. |
| `cleaners/*.toml` | Built-in cleaner definitions. `build.rs` embeds them as `BUILTIN_CLEANERS`. |
| `build.rs` | Generates the built-in cleaner table; reruns when `cleaners/` changes. |
| `tests/` | Reserved for integration tests (currently empty; unit tests live beside the code in `#[cfg(test)]` blocks). |
| `docs/` | This documentation. `docs/usage/` is the user-facing guide. |
| `bench/`, `dist/`, `target/` | Benchmarks, packaged output, build artifacts — not source. |

## `src/` layout

| Area | Role | Start here |
| --- | --- | --- |
| `core/` | Errors, path rules, the keep guard, reports. | `keep.rs` (gate 1: `Guard::check`), `path.rs` (normalize/compare), `report.rs` |
| `action/` | Executes a resolved action. | `file.rs` (delete pipeline), `safety.rs` (gate 2: `check_path`), `system.rs` (special/privileged actions) |
| `engine/` | Resolves selectors and drives actions. | `worker.rs` (`resolve`, `run`), `backup.rs`, `trash.rs`, `undo.rs` |
| `definition/` | Cleaner formats and registry. | `model.rs` (`CleanerDef`), `native.rs` (TOML), `mod.rs` (`CleanerRegistry`) |
| `cli/` | Clap commands and output. | `mod.rs` (`Cli`, `Command`, `execute`), `maintenance.rs` |
| `fsutil/` | Filesystem primitives. | `walk.rs` (traversal), `delete.rs` (low-level delete), `size.rs`, `reflink.rs` |
| `deep/` | Deep-scan cleaners. | `cleaners.rs` |
| `platform/` | OS-specific helpers. | `mod.rs`, then `windows.rs` / `unix.rs` / `linux.rs`, `winreg.rs` |
| `config/` | Settings load/save. | `mod.rs` (`Config`) |
| `shred/` | Secure overwrite. | `overwrite.rs` |
| `i18n.rs` | Language setting; English strings only today. | — |

## Common changes

- **Add a built-in cleaner:** drop a TOML file in `cleaners/`, then check
  `docs/cleaner-format.md`. `build.rs` picks it up on the next build.
- **Add a CLI command:** `cli/mod.rs` — add the variant to `Command` and its
  branch in `execute`; put bulky maintenance work in `cli/maintenance.rs`.
- **Change deletion safety:** both gates matter — `core/keep.rs` (gate 1) and
  `action/safety.rs` (gate 2). `fsutil/delete.rs` does **not** guard on its own;
  callers must pass a checked path. Add a regression test beside the change.
- **Support another cleaner format:** `definition/{native,cleanerml,winapp2}.rs`
  parse into `CleanerDef`; register it in `definition/mod.rs`.
- **Change report output:** `core/report*.rs`; keep stdout to one document so
  `--json` consumers do not break (see `docs/usage/04-automation.md`).

## Build and test

```sh
cargo test --all-targets
```

If the MSVC toolchain has no `link.exe`, use the GNU toolchain instead:
`cargo +1.88-x86_64-pc-windows-gnu test --all-targets`.

Unit tests live in the module they test; cross-platform integration tests go in
`tests/`. Link/junction tests use the `crate::junction` test helper and skip
gracefully when the platform refuses.
