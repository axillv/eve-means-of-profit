# eve-means-of-profit

A desktop tool for EVE Online traders and industrialists: prices, hauling, build planning. Written in Rust with egui.

Early development; nothing usable yet.

## Build
```
cargo build --release
cargo test
```

## Contributing
Enable the git hooks once per clone:
```
git config core.hooksPath .githooks
```
Commits then run `cargo fmt --check` and `cargo clippy` with warnings denied, and pushes run `cargo test`.
