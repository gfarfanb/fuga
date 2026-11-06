# fuga

Local-first Markdown notes: plain `.md` files in configurable source directories,
with a rebuildable SQLite index for backlinks, tags, and full-text search. See
[SPEC.md](SPEC.md) for the full design.

Two binaries ship:

- `fuga` — the headless server/CLI (`crates/fuga-cli`)
- `fuga-tui` — the terminal client (`crates/fuga-tui`)

**Status: M0 (skeleton).** The workspace, CI, config parsing/validation, and TUI
terminal setup/teardown with a panic hook exist. There is no server, index, or real
TUI layout yet — `fuga-tui` draws a placeholder.

## Requirements

- Rust **stable** (1.82 or newer). Install with [rustup](https://rustup.rs):
  ```sh
  rustup toolchain install stable
  ```
- No C toolchain needed (SQLite is bundled when the index lands in M1).
- Windows: use Windows Terminal (or any VT-capable terminal). Legacy `cmd.exe` is unsupported.
- Optional for `cargo fmt`/`cargo clippy` (the CI commands below):
  ```sh
  rustup component add rustfmt clippy
  ```

## Build

```sh
cargo build --workspace          # debug build of every crate
cargo build --release            # optimized (LTO)
```

## Run

```sh
# CLI: loads config (or defaults) and prints the source count
cargo run -p fuga-cli
cargo run -p fuga-cli -- --config /path/to/config.toml

# TUI: enters the alternate screen; press q, Esc, or Ctrl-C to quit
cargo run -p fuga-tui
```

The built binaries are `target/debug/fuga` and `target/debug/fuga-tui`.

## Config

Config is read once at startup from the platform config dir (SPEC §11):

| Platform | `config.toml` | Data dir (index, session) |
|----------|---------------|---------------------------|
| Linux | `~/.config/fuga/config.toml` | `~/.local/share/fuga` |
| Windows | `%APPDATA%\fuga\config\config.toml` | `%APPDATA%\fuga\data` |
| macOS (best effort) | `~/Library/Application Support/fuga/config.toml` | `~/Library/Application Support/fuga` |

A missing file is fine — defaults apply. `--config <path>` overrides the location.
Source names are validated at load (empty, `.`/`..`, separators, `<>:"/\|?*[]#`, and
Windows device names like `NUL` are rejected; duplicates are an error).

Minimal example:

```toml
[[sources]]
name = "notes"
path = "~/notes"
capture_dir = "inbox"
auto_write_id = true
exclude = ["archive/**"]
watch_mode = "auto"        # "auto" | "native" | "poll"
```

See SPEC.md §11 for every option (`[server]`, `[client]`, `[editor]`, `[index]`,
`[ui]`, `[keymap]`). Fields not listed keep their defaults; only `name` and `path`
are required per source.

## Test

```sh
cargo test --workspace           # everything
cargo test -p fuga-core          # one crate
cargo test -p fuga-core config   # one module / name filter
cargo test --workspace -- --nocapture   # show test stdout
```

## Lint and format (same as CI)

```sh
cargo fmt --all                  # format
cargo fmt --all --check          # CI check
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs the three commands above on `ubuntu-latest` and `windows-latest` for every
push to `main` and every pull request.

## Debug

```sh
RUST_BACKTRACE=1 cargo run -p fuga-tui      # full backtrace on panic
cargo build --workspace && RUST_BACKTRACE=full ./target/debug/fuga
```

- A panic in the TUI restores the terminal (raw mode off, alternate screen left)
  before the backtrace prints, so a crash never leaves the terminal broken.
- The TUI owns the terminal while running; if it is killed without unwinding
  (e.g. `SIGKILL`), run `reset` to recover the terminal.
- `cargo expand` / `cargo tree -p <crate>` help when inspecting dependencies.
- There is no file logging yet; `tracing` logging lands with the server (M2).
