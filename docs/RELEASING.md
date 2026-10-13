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
4. Build the packages on each OS (or cross-compile):

   ```sh
   scripts/release.sh                               # this machine
   scripts/release.sh x86_64-unknown-linux-gnu x86_64-pc-windows-gnu   # from Linux
   scripts/release.sh aarch64-apple-darwin x86_64-apple-darwin         # on a Mac
   ```

   Everything lands in `dist/` (not committed) with `SHA256SUMS`:

   | OS | Files |
   |---|---|
   | Windows | `veta-X.Y.Z-windows-x64.exe` (installer), `veta-X.Y.Z-x86_64-pc-windows-gnu.zip` (portable) |
   | Linux | `Veta-X.Y.Z-x86_64.AppImage`, `veta-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
   | macOS | `Veta-X.Y.Z-<arch>.dmg`, `veta-X.Y.Z-<arch>-apple-darwin.tar.gz` |

   Archives hold `veta`, `veta-cli`, `README.md`, `LICENSE` and
   `CHANGELOG.md`.

   Tools: Windows from Linux needs `mingw-w64` (`rustup target add
   x86_64-pc-windows-gnu`) and `nsis`; the AppImage needs `appimagetool`
   (`$APPIMAGETOOL`, on `PATH`, or `~/.cache/veta-tools/appimagetool`); the
   `.dmg` needs macOS. A missing tool skips that package with a warning.
5. Try the archives on each OS, then merge `dv` into `pr`, tag `vX.Y.Z` and
   publish a GitHub release with the archives and the changelog section.

## Packages

- **Windows installer** (`packaging/windows/veta.nsi`): installs for the
  current user into `%LOCALAPPDATA%\Programs\Veta` without admin rights,
  adds a Start menu entry, an uninstaller (Settings → Apps), and optionally
  adds the folder to the user `PATH` for `veta-cli`. Settings in
  `%APPDATA%\veta` survive uninstalling. `veta.exe` carries the icon and
  version info (file version `X.Y.Z.BUILD`) from `crates/veta/build.rs`.
- **AppImage**: one file; make it executable and run it. A link named
  `veta-cli` to it runs the command line. Built against this machine's
  glibc, so it needs a distribution at least as new as the build machine
  (built on Ubuntu 24.04: glibc 2.39).
- **macOS** (`packaging/macos/make-app.sh`): unsigned, ad-hoc signed
  `Veta.app` in a `.dmg`; on first launch use right-click → Open.
  `veta-cli` is inside the bundle (`Veta.app/Contents/MacOS/veta-cli`).
- **Icon**: edit `packaging/icons/veta.svg`, then run
  `packaging/icons/render.sh` and commit the outputs.
