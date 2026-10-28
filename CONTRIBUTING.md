# Contributing

Thanks for helping! Small fixes and clear bug reports are welcome.

1. Open an issue first for anything big, so we can agree on the plan.
2. Fork the repo and make a branch.
3. Before you push, run:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

4. Add a test for any change in behavior. Tests live in `tests/relay.rs` and run the real program in temporary folders.
5. Keep the safety rules intact: no network listener, private file modes, size and rate caps.
6. Open a pull request and say what changed and why.

Never include tokens, databases, or personal paths in an issue or pull request.
