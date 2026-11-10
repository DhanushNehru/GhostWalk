# GhostWalk

A data-oriented headless web engine in Rust, built for AI agents, scraping, and automation.

GhostWalk fetches a page, parses it into a **flat arena DOM** (no pointer-chasing, no borrow-checker fights), runs its inline JavaScript in a bounded, sandboxed engine, and hands agents what they actually want: the DOM tree, readable text, or **semantic Markdown** - at a fraction of headless Chromium's cost.

> Status: **working MVP**. The fetch -> parse -> execute-inline-JS -> dump pipeline runs today (see below). Style, layout, rendering, CDP and MCP are designed and roadmapped - see [ARCHITECTURE.md](ARCHITECTURE.md) and [ROADMAP.md](ROADMAP.md). Contributions welcome: check the [open issues](../../issues).

## Quickstart

```console
$ cargo build --release
$ ./target/release/ghostwalk https://example.com --dump md
$ ./target/release/ghostwalk examples/demo.html --dump tree --stats
```

```
USAGE:
    ghostwalk <URL | FILE> [OPTIONS]

OPTIONS:
    --dump <FORMAT>       tree (default) | text | md | json
    --no-js               skip inline <script> execution
    --js-loop-limit <N>   max JS loop iterations per engine (default 5000000)
    --timeout-ms <N>      total fetch timeout (default 10000)
    --max-bytes <N>       response body ceiling (default 5242880)
    --user-agent <UA>     User-Agent header
    --stats               print stage timings and peak RSS to stderr
```

## What works today

- HTML5 parsing via [html5ever](https://github.com/servo/html5ever) (the Servo project's standards-track parser) into ghostwalk's own arena storage
- Live DOM bridge for JavaScript: `document.querySelector(All)`, `document.title`, `document.body`, element `textContent` get/set, `get/setAttribute` - JS mutations are visible in the dumps
- Bounded JS execution: [Boa](https://github.com/boa-dev/boa) engine with a loop-iteration budget that kills runaway scripts, recursion/stack limits, and no network or file access from page JS. This is resource bounding, **not** a security sandbox yet - see the security model in [ARCHITECTURE.md](ARCHITECTURE.md)
- Bounded fetching: timeout, redirect cap, byte ceiling on every request
- Agent output: DOM tree dump, readable text, semantic Markdown, or JSON
- Tests: `cargo test` (unit + end-to-end pipeline)

## Measured (MVP, release build)

Single 2-core Linux sandbox, best of 3 runs. Method matters, so it is named per metric:

| Metric | Measured | Method |
| --- | --- | --- |
| Process start-to-exit | ~1-2 ms | wall time of `ghostwalk --version` |
| Full pipeline on a local file | ~1 ms | wall time of `ghostwalk examples/demo.html --dump text` |
| Peak RSS during that pipeline run | ~8.5 MiB | `VmHWM` from `/proc/self/status` |
| Stripped release binary | ~12 MB | `ls` on `target/release/ghostwalk` |

The project-level targets are sub-30 ms startup and sub-50 MiB idle RSS for the full engine (with rendering); the numbers above cover only the current MVP pipeline, not an idle browser process. These are single-machine measurements, not guarantees - the benchmark harness that tracks them across a real corpus is an [open issue](../../issues).

## What ghostwalk is not (yet)

- Not a full browser: no CSS cascade, no layout, no pixels yet
- Not a Chromium drop-in: the planned CDP support is a documented subset
- Not a network-capable JS sandbox: page scripts cannot fetch, and external `<script src>` is skipped in the MVP

## Why "GhostWalk"?

A ghost walks without a body. GhostWalk walks the web the same way - no window, no pixels-by-default, just the structure and content an agent needs.

## Project layout

| Crate | Role |
| --- | --- |
| `crates/ghostwalk-dom` | Flat arena DOM, html5ever sink, selector subset, text/Markdown/tree extraction |
| `crates/ghostwalk-net` | Bounded HTTP(S) fetching |
| `crates/ghostwalk-js` | JS runtime + live DOM bridge (Boa today, engine-swappable) |
| `crates/ghostwalk-cli` | The `ghostwalk` binary: fetch -> parse -> run JS -> dump |

Design decisions, the JS-engine comparison, and the roadmap to actors, rendering, CDP and MCP: [ARCHITECTURE.md](ARCHITECTURE.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Good first issues are labeled [`good first issue`](../../issues).

## License

MIT - see [LICENSE](LICENSE).
