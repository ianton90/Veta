# Veta
A Parquet file viewer and editor written in Rust, with a desktop GUI (iced) and a CLI.

Status: early development. See [docs/SCOPE.md](docs/SCOPE.md) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build and run

Requires stable Rust 1.88 or newer.

```sh
cargo run --release            # open the GUI
cargo run --release -- --help  # CLI help
```

Linux needs the usual desktop libraries at runtime (present on any desktop
install): `libxkbcommon-x11` for X11, Wayland libraries for Wayland, and a
Vulkan or OpenGL driver.

## Development

Sample files for trying the app by hand:

```sh
cargo run -p veta-testkit --example fixtures -- /tmp/veta-fixtures
cargo run -- open /tmp/veta-fixtures/*.parquet
```

Before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License
Apache-2.0. See [LICENSE](LICENSE).
