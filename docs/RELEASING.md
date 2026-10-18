# Releasing

## Version numbers

- The version is `MAJOR.MINOR.PATCH` ([SemVer](https://semver.org)), set
  once in the root `Cargo.toml` (`[workspace.package] version`). It appears
  in tags (`v0.3.0`), archive names, About and `--version`.
  - PATCH: fixes only. MINOR: new features. MAJOR: breaking changes (file
    formats such as recipes, CLI options). While MAJOR is 0, MINOR may also
    break things.
  - Pre-releases: `0.3.0-beta.1`.
- Every build also has a **build number**: `$VETA_BUILD_NUMBER` if set,
  otherwise the commit count of the checkout. It and the short commit hash
  are shown next to the version, e.g. `0.3.0 (build 51, 341f5c6)`, and will
  go into the Windows file version's fourth field and the macOS build
  number once installers exist (#51).

## Steps

1. Start from `dv` with a clean tree. Run the checks from `CLAUDE.md`
   (fmt, clippy, tests).
2. Bump the version in `Cargo.toml` (and `Cargo.lock` via `cargo build`).
   Move the notes under a new heading in `CHANGELOG.md`.
3. Commit (`Release 0.3.0`) and push to `dv`.
4. Push the tag `vX.Y.Z`. GitHub Actions (`.github/workflows/release.yml`;
   kept in `packaging/ci/release.yml` until it is copied there, since
   pushing workflow files needs a token with the `workflow` scope)
   builds every package on Linux, Windows and macOS and creates a **draft**
   release with them, `SHA256SUMS` and the changelog section. It can also
   be run by hand (Actions → Release → Run workflow); the packages are then
   the run's artifacts. To build locally instead:

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
5. Try the packages on each OS, then publish the draft release and merge
   `dv` into `pr`.

## Packages

- **Windows installer** (`packaging/windows/veta.nsi`): installs for the
  current user into `%LOCALAPPDATA%\Programs\Veta` without admin rights,
  adds a Start menu entry, an uninstaller (Settings → Apps), and optionally
  opens `.parquet`/`.parq` files with Veta and adds the folder to the user
  `PATH` for `veta-cli`. If another app was already chosen for `.parquet`,
  Windows keeps it until the user picks Veta (Open with → Always). Settings in
  `%APPDATA%\veta` survive uninstalling. `veta.exe` carries the icon and
  version info (file version `X.Y.Z.BUILD`) from `crates/veta/build.rs`.
- **AppImage**: one file; make it executable and run it. A link named
  `veta-cli` to it runs the command line. `Veta.AppImage --integrate` adds
  the menu entry, icon and the `application/vnd.apache.parquet` type to
  `~/.local/share` and makes Veta the default for it;
  `--remove-integration` undoes it. The release workflow builds it on
  Debian 11 (glibc 2.31), so it runs on Ubuntu 20.04, Debian 11 and newer;
  a local build needs a distribution at least as new as the build machine.
- **macOS** (`packaging/macos/make-app.sh`): unsigned, ad-hoc signed
  `Veta.app` in a `.dmg`; on first launch use right-click → Open.
  `veta-cli` is inside the bundle (`Veta.app/Contents/MacOS/veta-cli`).
  `Info.plist` declares `.parquet` (`org.apache.parquet`), so Finder offers
  Veta for it; Finder sends files as Apple Events, which Veta doesn't read
  yet (#84).
- **Icon**: edit `packaging/icons/veta.svg`, then run
  `packaging/icons/render.sh` and commit the outputs.
