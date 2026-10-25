# Contributing

- Build: `cargo run --release` (Linux needs `libasound2-dev pkg-config`).
- Before opening a pull request run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
- Keep the UI readable at 1280×720 and never force window sizes — RadioMaster remembers the user's window geometry.
- Station-specific addresses, logins or company resources must never be built in; web panels and AzuraCast are configured by the user.

## Checks on every pull request

CI (`.github/workflows/ci.yml`) must be green before a merge. The **CI OK** check sums it up, so it is
the one to require in branch protection. It runs:

- `cargo fmt --check`, and `cargo clippy -- -D warnings` on Linux, macOS and Windows (a compiler warning fails too)
- `cargo test` on Linux and macOS, and the release build plus installer on Windows
- RadioMaster Server built on Ubuntu 22.04 (the same as the release) and started, pinged and signed in to (`tools/smoke_server.sh`)
- the version is the same in `Cargo.toml`, `Cargo.lock`, `installer/RadioMaster.iss` and the newest CHANGELOG section (`tools/check_versions.py`)
- the Updates page and `lang/template.json` are regenerated and must match what's committed
- every icon character can be drawn (`tools/check_glyphs.py`), and every website link and image resolves (`tools/check_site.py`)
- the workflow files themselves (actionlint)

CI uses the fixed Rust version in `RUST_VERSION` at the top of `ci.yml`, so a new Rust release can't turn
every pull request red overnight. Bump it on purpose and fix what the newer clippy finds in the same pull request.

`cargo audit` (`.github/workflows/audit.yml`) runs when the dependencies change and every Monday.
A tag push checks the tag against the version before the release builds anything.

## MergeGuard

The MergeGuard bot reviews each pull request for logic and risk; CI proves the code builds, lints and
passes its tests. They work together like this:

- Treat a MergeGuard finding as a question. Check it against the code and the CI result: a claim
  that something is undefined, unused or won't compile is already settled by a green clippy run.
- Fix the real findings. For a false one, reply on the thread with the evidence and resolve it.
- Don't apply its auto-fix suggestions without reading them; CI runs again on the fix either way.
- After pushing fixes, comment `@mergeguard-followup` so it reviews the new diff. For risky changes
  (audio engine, server, TLS, the release workflow) ask for `@mergeguards deep-scan`.
