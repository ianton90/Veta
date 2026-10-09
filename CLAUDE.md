# Veta — notes for Claude

- Read `docs/SCOPE.md`, `docs/ARCHITECTURE.md` and `docs/DECISIONS.md` first.
- Develop on branch `dv`; push directly. `pr` is the release branch, do not push there.
- Tasks are GitHub issues in `ianton90/veta` (parent issue per milestone, sub-issues as tasks). Reference the issue in commits (`#N`) and close it when done.
- No CI. Before every push run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Keep dependencies minimal. New crates need an entry in `docs/DECISIONS.md` and the owner's approval.
- `veta-core` must not depend on `iced` or `clap`. All data changes go through controller `Command`s.

## How we work

- The owner reviews code but has little time: take the lead, keep answers short, and explain only when asked.
- Make one commit per task (issue), not one per milestone.
- Work one milestone at a time. When it's done, report what changed and any gaps, then ask before starting the next one.
- Commits on `dv` don't auto-close issues (only the default branch does). Close each finished issue, and its milestone parent, by hand with reason "completed".
- Gaps, tech debt or untested parts found along the way become sub-issues of #53 "Follow-ups". Notes that only matter to an existing task go as a comment on that issue.
- Ask before adding dependencies or making decisions that change scope; record decisions in `docs/DECISIONS.md`.

## Checking the GUI in a cloud session

Cloud containers have no display. To see the app:

```sh
apt-get install -y libxkbcommon-x11-0 xdotool        # once per container
cargo run -p veta-testkit --example fixtures -- /tmp/fx
xvfb-run -a -s "-screen 0 1280x800x24" bash -c \
  './target/debug/veta open /tmp/fx/*.parquet & P=$!; sleep 12; import -window root /tmp/shot.png; kill $P'
```

Use `xdotool` (mousemove, click, key) inside the `xvfb-run` script to interact; first give the window keyboard focus with `xdotool windowfocus $(xdotool search --name Veta | head -1)` (there is no window manager). Take screenshots with `import`, and look at them before reporting GUI work as done. Drag and drop can't be tested this way.
