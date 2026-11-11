# Cleaner definition format (native TOML)

Authoritative for `src/definition/native.rs`. Built-in cleaners live
under `cleaners/`; drop-ins load via `--define DIR`. BleachBit XML and
`winapp2.ini` definitions load through `--bleachbit` and `--winapp2`
and are normalized into the same cleaner model.

## Top level

```toml
id = "demo"
name = "Demo cache"
description = "Disposable fixture"
os = "windows"          # optional; empty = any

[[running]]
type = "exe"            # "exe" | "path"
value = "demo.exe"
os = "windows"          # optional
same_user = false       # optional (exe only)

[[var]]
name = "cache_dir"

[[var.value]]
value = "C:/Users/you/cache"
search = "literal"      # literal | glob | winreg | registry
os = "windows"          # optional
name = "browser"        # registry name override

[[option]]
id = "cache"
label = "Cached logs"
description = "Delete the fixture cache tree"
warning = "Runs at startup"  # optional
os = "windows"                # optional

[[option.action]]
command = "delete"
search = "walk.all"           # file | glob | walk.all | walk.files | walk.top | deep
path = "$$cache_dir$$/logs"
os = "windows"                # optional
regex = "\\.log$"             # optional
nregex = "keep"               # optional
wholeregex = "sweepdocs"      # optional
nwholeregex = "critical"      # optional
type = "file"                 # f|file = files, d|dir|directory = dirs
max_age_days = 1              # optional
min_age_days = 1              # optional
min_size = "10KB"             # optional; integer or human size
max_size = "1MB"              # optional
max_depth = 3                 # optional
include_dirs = true           # optional
section = "Foo"               # ini only
parameter = "key"             # ini only
address = "a/b"               # json only, slash-separated
cmd = "apt-get clean"         # process only
wait = true                   # process only, default true
reg_name = "Value"            # winreg only
exclude_keys = ["A", "B"]     # winreg only
```

## Rules

- `id` is required and unique; option `id`s are unique per cleaner.
- Default `search`: `glob` if `path` contains `* ? [ ] {`, else
  `file`. Empty `command` defaults to `delete`; empty `os` is any;
  `wait` defaults to `true`.
- Empty `regex` / `reg_name` become `None`.
- Rejected at load (validation warnings, definition skipped):
  unknown `search`/`type`/`running.type`/`var.search`, duplicate
  `var` names, duplicate option `id`s, `min_size > max_size`,
  `min_age_days > max_age_days`, unknown `os` filter, undeclared
  `$$var$$` placeholders, unknown action command, path-requiring
  command with empty path.
- Variable syntax: `$$name$$` only, exact boundary (`$$a_b$$` is
  not `$$a$$`). Cartesian expansion over all bound values.
- Human sizes: integer, or number + suffix (`KB`,`MB`,… binary
  `KiB`,`MiB`,…; decimal `k`/`m`/…). `100KB` = 100000.
- Unknown OS filters make the cleaner match nothing — `validate()`
  warns, because a typo used to silently skip every system.

## Provider check

`validate_action` runs per action: unknown command fails closed;
commands with `needs_path` (e.g. `delete`, `ini`, `json`,
`sqlite.vacuum`) must have a non-empty `path`;
`winreg`/`process` are exempt from the filesystem-path rule.
`action/provider.rs` (81 providers) is the table — summaries,
destructive flag, `windows_only`/`unix_only`, `privileged`.

## Trust

`LoadOptions::external_trust` defaults to `Untrusted`: privileged
commands (`winreg`, `process`, `apt.autoremove`) are stripped from
external definitions; builtins are `Trusted`. A trusted cleaner
always replaces an untrusted one with the same id; same trust, last
writer wins. Pass `--trust-external` to load external definitions
with full power.

## Example worth copying

```toml
id = "demo"
name = "Demo cache"
[[option]]
id = "cache"
label = "Cached logs"
[[option.action]]
command = "delete"
search = "walk.all"
path = "/tmp/sweepdocs/cache"
```
