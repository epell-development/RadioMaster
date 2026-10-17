# Contributing

- Build: `cargo run --release` (Linux needs `libasound2-dev pkg-config`).
- Before opening a pull request run `cargo fmt` and `cargo test`.
- Keep the UI readable at 1280×720 and never force window sizes — RadioMaster remembers the user's window geometry.
- Station-specific addresses, logins or company resources must never be built in; web panels and AzuraCast are configured by the user.
