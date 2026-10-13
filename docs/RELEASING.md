# Releasing

## Version numbers

- The version is `MAJOR.MINOR.PATCH` ([SemVer](https://semver.org)), set
  once in the root `Cargo.toml` (`[workspace.package] version`). It appears
  in tags (`v0.1.0`), archive names, About and `--version`.
  - PATCH: fixes only. MINOR: new features. MAJOR: breaking changes (file
    formats such as recipes, CLI options). While MAJOR is 0, MINOR may also
    break things.
  - Pre-releases: `0.2.0-beta.1`.
- Every build also has a **build number**: `$VETA_BUILD_NUMBER` if set,
  otherwise the commit count of the checkout. It and the short commit hash
  are shown next to the version, e.g. `0.1.0 (build 51, 341f5c6)`, and will
  go into the Windows file version's fourth field and the macOS build
  number once installers exist (#51).

## Steps

1. Start from `dv` with a clean tree. Run the checks from `CLAUDE.md`
   (fmt, clippy, tests).
2. Bump the version in `Cargo.toml` (and `Cargo.lock` via `cargo build`).
   Move the notes under a new heading in `CHANGELOG.md`.
3. Commit (`Release 0.2.0`) and push to `dv`.
4. Build the archives on each OS (or cross-compile):

   ```sh
   scripts/release.sh                               # this machine
   scripts/release.sh x86_64-pc-windows-gnu         # from Linux, needs mingw-w64
   scripts/release.sh aarch64-apple-darwin x86_64-apple-darwin   # on a Mac
   ```

   Archives and `SHA256SUMS` land in `dist/` (not committed). Each holds
   `veta`, `veta-cli`, `README.md`, `LICENSE` and `CHANGELOG.md`.
5. Try the archives on each OS, then merge `dv` into `pr`, tag `vX.Y.Z` and
   publish a GitHub release with the archives and the changelog section.
