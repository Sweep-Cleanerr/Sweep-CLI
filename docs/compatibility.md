# Compatibility

Coverage of the 209 embedded cleaners against Sweep's providers.
Derived from `src/action/provider.rs` (81 providers) and the
loader/OS gates in `src/definition/`. Runtime-tested only where a test
exercises it; `cargo test --all-targets` reports 240 tests.

## Provider families

| Family | Commands | Status |
| --- | --- | --- |
| File delete | `delete`, `truncate` | working (`action/file.rs`) |
| Shred | `shred` | working (`shred/overwrite.rs`) |
| Structured | `ini`, `json` | working (`action/structured.rs`) |
| SQLite | `sqlite.vacuum`, `cookies`, `chrome.*`, `mozilla.*` | working (`action/sqlite.rs`) |
| System machine-wide | `system.clipboard`, `system.dns`, `system.memory`, `system.journald`, `system.font_cache`, `system.recycle_bin`, `system.trim`, `system.free_disk_space` | working (dry-run reports, real on `--yes` where the platform supports it) |
| System directory cleaners | `system.trash`, `system.tmp`, `system.rotated_logs`, `system.localizations`, `system.recent_documents`, `system.custom`, `system.prefetch`, `system.wer`, `system.windows_update_cache`, `system.delivery_optimization`, `system.explorer_caches`, `system.thumbcache`, `system.store_cache`, `system.d3d_shader_cache`, `system.crash_dumps`, `system.memory_dumps` | working via Guard + safety |
| Process / registry / packages | `process`, `winreg`, `apt.clean`, `apt.autoclean`, `apt.autoremove`, `dnf.clean`, `yum.clean`, `pacman.clean`, `zypper.clean`, `brew.cleanup` | working (Windows-only items fail clearly elsewhere) |
| Deep engine | `system.apt_cache`, `system.dnf_cache`, `system.pacman_cache`, `system.old_kernels`, `system.journal_vacuum`, `system.user_caches`, `system.temp_deep`, `system.orphans`, `system.snap`, `system.flatpak`, `system.apt_lists`, `system.zypp_cache`, `system.crash_reports`, `system.coredumps`, `system.docker`, `system.docker_volumes`, `system.kde_cache`, `system.kde_pim`, `system.baloo`, `system.gnome_cache`, `system.zeitgeist`, `system.tracker`, `system.nix_gc`, `system.tm_thin`, `system.journal_user`, `system.dev_npm`, `system.dev_pip`, `system.dev_cargo`, `system.dev_gradle`, `system.dev_go` | working through `deep/cleaners.rs` (Guard + safety, tool errors reported) |

## OS gates

- `os = "windows"` cleaners skip on non-Windows (and vice versa).
- `system.recycle_bin`, `system.prefetch`, `system.wer`,
  `system.windows_update_cache`, `system.explorer_caches`,
  `system.thumbcache`, `system.store_cache`, `system.d3d_shader_cache`,
  `system.crash_dumps`, `system.memory_dumps` are Windows-only.
- `system.journald`, `system.rotated_logs`, `system.localizations`,
  `system.font_cache`, package managers are Unix/Linux-only.
- `winreg` is Windows-only and privileged; `process` is
  privileged and stripped from untrusted definitions.

## Not runtime-tested here

- Linux/macOS platform helpers are compiled but were not exercised in
  the Windows test session: `platform/unix.rs` (`trash_dirs`,
  `rotated_logs`, `localization_paths`), `platform/linux.rs` (`fitrim`,
  `wipe_memory`, `journald_clean`). Run `cargo test --all-targets` on
  those platforms before relying on them.
- Windows symlink creation needs elevation; the junction fallback is
  tested and visible in test output.
- Deep-engine tooling (docker, flatpak, journald, package managers)
  and the maintenance helpers (scheduler, process-name detection,
  OS recycle-bin deletion, persistent index) are best-effort and
  have not been exercised against every target platform.
