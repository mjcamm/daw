# CLAUDE.md

A DAW where movement is the core of the music and automation is the core of the app.
Read `daw-foundation.md` (vision, principles, concepts) and `build-plan.md` (phases) before starting work.

## Commands

```sh
./scripts/check.sh                           # everything below, plus release builds; must pass before committing
cargo test --workspace                       # all tests (debug: realtime guard active)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
DAW_BLESS=1 cargo test -p <crate> --test <t> # write/update reference audio
```

There is no hosted CI; checks run locally. Run `./scripts/check.sh` before every commit.

## Crates

- `daw-protocol`: engine/UI boundary. Commands in, snapshots out. Depends on nothing; a front end depends only on this.
- `daw-engine`: authoritative state. `Engine` (control thread: commands, undo, snapshots) and `AudioProcessor` (audio thread), connected only by lock-free `rtrb` queues.
- `daw-dsp`: standalone, portable DSP modules. No engine knowledge.
- `daw-rt`: realtime safety guard (`GuardedAllocator`, `realtime`, `count_violations`).
- `daw-testkit`: headless offline render (`render`), reference audio comparison (`assert_reference!`). Linking it installs the guard.

## Rules

- **Test-driven.** Write the failing test first. Prefer pure functions for automation/layer logic.
- **Audio thread never allocates, frees, locks or blocks.** Audio-side code runs inside `daw_rt::realtime`; debug builds abort on any allocation there. Replaced state goes back to the control side through a garbage queue to be freed there. Locks and `thread::sleep` are banned by clippy workspace-wide; control-side code that needs one opts in with `#[allow(clippy::disallowed_types, reason = "...")]`.
- **One source of truth.** State lives in the engine. Front ends send `Command`s and render `Snapshot`s.
- **Every edit is undoable.** `Project::apply` returns the inverse edit; new edits go through `History`.
- **Live and offline render share one code path**: `AudioProcessor::process`.
- Every binary and test harness must install `daw_rt::GuardedAllocator` as the global allocator.
- Reference audio lives in `<crate>/tests/references/*.wav` (32-bit float). Only bless a change you have listened to or can explain.
