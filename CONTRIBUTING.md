# Contributing to ghostwalk

Thanks for helping build a headless web engine that treats agents as
first-class users. This project is early on purpose: the MVP is small,
the design space is large, and good contributions shape it.

## Ground rules

- Rust stable (1.85+). Everything must build with `cargo build` - no C/C++
  toolchain requirement is a feature, keep it.
- `cargo test` passes, `cargo fmt` applied, `cargo clippy` clean before
  review.
- Work on a branch, open a pull request. Nothing lands on `main` directly.
- Honest claims: implemented vs planned and measured vs target are always
  labeled. If you measure something, say how.
- Small PRs beat big PRs. One concern per PR.

## Getting started

```console
$ git clone https://github.com/DhanushNehru/ghostwalk
$ cd ghostwalk
$ cargo build
$ cargo test
$ cargo run -- examples/demo.html --dump tree --stats
```

## Where to start

- Issues labeled [`good first issue`](../../issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) are scoped for newcomers.
- [`help wanted`](../../issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22) marks work the project actively needs.
- Bigger design work (actors, CDP, MCP) starts as a short design comment on
  its issue before code.

## Pull request checklist

1. Branch from `main`: `git checkout -b feature/my-change`
2. Make the change with tests; add docs for new public behavior
3. `cargo fmt && cargo clippy --all-targets && cargo test`
4. Open the PR: what it does, why, how it was tested, and measurements if
   performance-relevant
5. Respond to review; squash-merge keeps history tidy

## Code style notes

- Arena-first: new DOM state goes into component stores keyed by `NodeId`,
  not into `Rc`/pointer graphs.
- No `unsafe` without a comment justifying it (the JS bridge documents its
  invariants today).
- Errors: `thiserror` in libraries, exit codes with messages in the CLI.
- Dependencies: additions need justification in the PR (build time and
  supply-chain size are user-facing costs here).

## Reporting bugs

Open an issue with the input HTML/URL, the command you ran, what you
expected, and what ghostwalk printed. Security-sensitive reports: please use
GitHub's private vulnerability reporting instead of a public issue.

## License

By contributing you license your work under the project's MIT license.
