# Configuration

Everything configurable today is an environment variable. The PRD sketches a
`~/.../nermo/config.toml` (§17.1) and per-project `.nermo/config.toml`
(§17.2) — **neither exists yet**; don't reference them as if they're
implemented. Environment variables were enough for the MVP and match the
PRD's own stated precedence rule (§17.3: env vars override file config).

## Environment variables

| Variable | Effect | Default |
|---|---|---|
| `NERMO_STORE` | Overrides the global store directory directly. | *(unset)* |
| `NERMO_HOME` | Overrides nermo's whole application-data directory; the store lives at `$NERMO_HOME/store`. Ignored if `NERMO_STORE` is also set. | *(unset)* |
| `NERMO_CONCURRENCY` | Max concurrent package downloads during `install` (`Store::ensure_all`). Non-numeric or `0` falls back to the default. | `16` |

## Default store location (when neither `NERMO_STORE` nor `NERMO_HOME` is set)

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/nermo/store` |
| Linux | `$XDG_DATA_HOME/nermo/store`, or `~/.local/share/nermo/store` if `XDG_DATA_HOME` is unset |
| Windows | `%LOCALAPPDATA%\Nermo\store` |

(`store::nermo_home` / `store::default_root` in `src/store.rs`.)

## Per-project configuration

None. `package.json` is read as-is (`dependencies` + `devDependencies`); the
only nermo-specific file a project gets is `.nermo-lock` (see
`docs/lockfile.md`). There's no way to override the registry, linker
strategy, or script-execution policy per project yet — the registry is
always `https://registry.npmjs.org`, and lifecycle scripts are never
executed (see `docs/compatibility.md`).
