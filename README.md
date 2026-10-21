# Veta
A Parquet file viewer and editor written in Rust, with a desktop GUI (iced) and a CLI.

Status: early development (0.6.0). See [CHANGELOG.md](CHANGELOG.md),
[docs/SCOPE.md](docs/SCOPE.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Two programs: `veta` is the desktop app (`veta [FILE]...`), `veta-cli` the
command line (`veta-cli --help`).

## Build and run

Requires stable Rust 1.88 or newer.

```sh
cargo run --release -p veta                  # open the desktop app
cargo run --release -p veta-cli -- --help    # command-line help
```

Release archives: `scripts/release.sh` (see [docs/RELEASING.md](docs/RELEASING.md)).

Linux needs the usual desktop libraries at runtime (present on any desktop
install): `libxkbcommon-x11` for X11, Wayland libraries for Wayland, and a
Vulkan or OpenGL driver.

## Settings and themes

Settings live in `config.toml` in Veta's config directory:

| OS | Directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/veta` or `~/.config/veta` |
| macOS | `~/Library/Application Support/veta` |
| Windows | `%APPDATA%\veta` |

Set `VETA_CONFIG_DIR` to use another directory. Custom themes are `.toml`
files in the `themes` subfolder; see
[the built-in light theme](crates/veta-gui/assets/themes/veta-light.toml) for
the format.

## Development

Sample files for trying the app by hand:

```sh
cargo run -p veta-testkit --example fixtures -- /tmp/veta-fixtures
cargo run -p veta -- /tmp/veta-fixtures/*.parquet
```

Before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License
Apache-2.0. See [LICENSE](LICENSE).
