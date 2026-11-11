# The working loop

Preview → clean → undo or purge. Paths in examples are illustrative;
shapes (counters, exit codes, refusal wording) are what to plan
against.

## Fixture

```toml
id = "demo"
name = "Demo cache"

[[option]]
id = "cache"
label = "Cached logs"

[[option.action]]
command = "delete"
search = "walk.all"
path = "/tmp/sweepdocs/data/cache"
```

```sh
sweep --define ./defs preview demo.cache
```

`--define DIR` (repeatable) loads definitions from `DIR`. For
state isolation use `--config PATH`, `--portable` (state next to the
executable), or set `SWEEP_PORTABLE=DIR`: without it, drop-in
definitions in the real hub/config tree join every `--all` run.

## 1. Preview first. Always.

```sh
sweep --define ./defs preview demo.cache
```

```text
Files removed: 2
Reclaimed: 36B
Special operations: 0
Skipped: 0
Errors: 0
Blocked: 0
Warnings: 0
Duration: 2 ms
```

Exit `0`. `preview` is always a dry-run: same resolver and walk as
`clean`, with the deletion step removed. With `--json` you get the
full entry list (see `03-reading-output`).

## 2. Clean — and the `--yes` gate

```sh
sweep --define ./defs clean demo.cache
```

Without `--yes` Sweep refuses and exits `1`, both files untouched:

```text
error: refusing to clean without --yes; run 'sweep preview' first
```

This is deliberate: Sweep will not infer consent from the absence
of a human. With a store the same run reports what it kept:

```sh
sweep --define ./defs --backup-dir ./backup clean demo.cache --yes
```

`net reclaimed 0B` is honest: copying to `--backup-dir` first
means the disk gained nothing yet. Space frees when the backup is
pruned (`sweep backup prune`). A clean without `--backup-dir`
reclaims immediately and restores nothing later.

## 3. Two ways to keep a way back

| | `--backup-dir DIR` | `--trash-dir DIR` |
| --- | --- | --- |
| Operation | **copy**, then delete | **rename** into `DIR` (same volume) |
| Space freed | on prune | on `purge` |
| Reclaim | `sweep undo --backup-dir DIR` | `sweep undo --backup-dir DIR` |
| Cost | I/O, 2× space | near-zero |

`--backup-max-size N` caps what gets copied; an oversized file is
deleted without a copy. The mirrored layout namespaces by drive
(`drive-c/...` on Windows) so volumes can't collide, and `purge`
refuses any directory without the `.sweep-trash` marker.

### Undo

```sh
sweep --backup-dir ./backup undo --last --yes
```

`--last` limits replay to the most recent run marker in the
operation log; `--run <marker>` picks a specific run. `undo`
overwrites whatever currently occupies the path, so it asks too.
Without `--backup-dir` it stops with an error rather than guessing.

### Purge

```sh
sweep --trash-dir ./trash purge            # preview by default
sweep --trash-dir ./trash purge --yes      # actually delete
sweep --trash-dir ./trash purge --older-than-days 30 --yes
```

Without the `.sweep-trash` marker, `purge` fails closed
("not a sweep trash dir"), exit `1`, nothing touched.

## 4. Check what you did

```sh
sweep history           # totals replayed from the operation log
sweep doctor            # config, tools, permissions, disk, services
sweep diagnostics       # detail for bug reports
```

## Safety rails you cannot turn off

- **Protected roots** (`C:\Windows`, `/etc`, `/usr`, …) are refused
  by the guard. `--force` only relaxes the running-application
  refusal; it does not lift protected roots.
- **The keep list** is a deny list the engine checks before
  deleting, regardless of selectors.
- **External definitions** lose `process` / `winreg` /
  `apt.autoremove` unless `--trust-external` is passed.

## Next

[02 · Choosing what to clean](02-selecting.md).
