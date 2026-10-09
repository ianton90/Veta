# Veta — notes for Claude

- Read `docs/SCOPE.md`, `docs/ARCHITECTURE.md` and `docs/DECISIONS.md` first.
- Develop on branch `dv`; push directly. `pr` is the release branch, do not push there.
- Tasks are GitHub issues in `ianton90/veta` (parent issue per milestone, sub-issues as tasks). Reference the issue in commits (`#N`) and close it when done.
- No CI. Before every push run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Keep dependencies minimal. New crates need an entry in `docs/DECISIONS.md` and the owner's approval.
- `veta-core` must not depend on `iced` or `clap`. All data changes go through controller `Command`s.
