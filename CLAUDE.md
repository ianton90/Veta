# Veta — notes for Claude

- Read `docs/SCOPE.md`, `docs/ARCHITECTURE.md` and `docs/DECISIONS.md` first.
- Develop on branch `dv`; push directly. `pr` is the release branch, do not push there.
- Tasks are GitHub issues in `ianton90/veta` (parent issue per milestone, sub-issues as tasks). Reference the issue in commits (`#N`) and close it when done.
- No CI on pushes (releases build in GitHub Actions on version tags, see `docs/RELEASING.md`). Before every push run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Keep dependencies minimal. New crates need the owner's approval, an entry in `docs/DECISIONS.md` saying why the exception (and what was measured, if it was for speed), and a short comment where the crate is used pointing to that entry.
- `veta-core` must not depend on `iced` or `clap`. All data changes go through controller `Command`s.

## Code conventions

- Group files in folders by purpose, at most two levels below `src/`; see the file map in `docs/ARCHITECTURE.md`. Start every file with a `//!` comment.
- Unit tests: inline `#[cfg(test)] mod tests` when short; a sibling `tests.rs` (or `tests/` folder) when over about 150 lines. Integration tests go in the crate's `tests/`.
- GUI: one file per dialog in `dialogs/`; dialogs that add or edit a step implement `StepDialog`. App code goes in the file of its area (`app/rows.rs`, …), ordered message handlers, ribbon actions, helpers.

## How we work

- The owner reviews code but has little time: take the lead, keep answers short, and explain only when asked.
- Make one commit per task (issue), not one per milestone.
- Work one milestone at a time. When it's done, report what changed and any gaps, then ask before starting the next one.
- Commits on `dv` don't auto-close issues (only the default branch does). Close each finished issue, and its milestone parent, by hand with reason "completed".
- Label every issue when creating it: `feature` (new functionality), `fixes` (works but not as it should), `bug`, `research`, `organization` (workflow, versioning, refactoring), `documentation`; `desirable`, `duplicate`, `wont do` as they apply.
- Gaps, tech debt or untested parts found along the way become sub-issues of #53 "Follow-ups". Notes that only matter to an existing task go as a comment on that issue.
- Ask before adding dependencies or making decisions that change scope; record decisions in `docs/DECISIONS.md`.

## Checking the GUI in a cloud session

Cloud containers have no display. To see the app:

```sh
apt-get install -y libxkbcommon-x11-0 xdotool        # once per container
cargo run -p veta-testkit --example fixtures -- /tmp/fx
xvfb-run -a -s "-screen 0 1280x800x24" bash -c \
  './target/debug/veta /tmp/fx/*.parquet & P=$!; sleep 12; import -window root /tmp/shot.png; kill $P'
```

Use `xdotool` (mousemove, click, key) inside the `xvfb-run` script to interact; first give the window keyboard focus with `xdotool windowfocus $(xdotool search --name Veta | head -1)` (there is no window manager). Take screenshots with `import`, and look at them before reporting GUI work as done. Drag and drop can't be tested this way.
