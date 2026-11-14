# Choosing what to clean

## Selectors

A selector names what to clean:

- `cleaner` — every active option of that cleaner
- `cleaner.option` — one option
- `*` or `all` — every active option of every cleaner
- globs (`fire*`, `*.cache`) — matched by name, case-insensitive

```sh
sweep preview system.tmp
sweep preview "firefox.*"
sweep preview --all --exclude "system.dns"
```

Unknown selectors fail fast (`cleaner not found` / `option not
found`) and nothing runs. Duplicates are de-duplicated in order;
an empty selection is `nothing selected`.

## Platform and OS filters

`list --os windows` shows only cleaners whose `os` filter matches
the target. At run time, actions whose `os` doesn't match the host
are skipped before execution. `list --running` shows only cleaners
whose `running` check currently matches: either a `running.path`
pattern exists or a `running.exe` process is running.

## Definitions

- Built-in: 209 TOML files under `cleaners/`, embedded at
  build time (`build.rs`). 139 are active on Windows after OS
  filtering.
- Drop-in: `--define DIR` (repeatable) loads `.toml` files from
  `DIR`. The same files work with `import`.
- `--bleachbit` and `--winapp2` auto-load BleachBit definitions and
  `winapp2.ini` from their standard locations, converting them to the
  native cleaner model.
- Trust: external definitions lose `process` / `winreg` /
  `apt.autoremove` unless `--trust-external` is passed. Unknown
  commands fail closed in `validate_action`.

## list / info / import

```sh
sweep list                        # all cleaners + options
sweep list firefox --search cache # substring filters
sweep list --os windows
sweep info firefox                # one cleaner, described
sweep import ./my-cleaner.toml    # validate a file, print --define hint
```

`import` parses and validates without registering anything; the
output tells you the exact `--define` flag to use.
