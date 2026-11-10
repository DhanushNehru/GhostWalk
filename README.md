<h1 align="center">👻 GhostWalk</h1>

<p align="center"><strong>A data-oriented headless web engine in Rust, built for AI agents</strong></p>

<p align="center">
  <a href="https://github.com/DhanushNehru/GhostWalk/stargazers"><img src="https://img.shields.io/github/stars/DhanushNehru/GhostWalk?style=flat-square&color=gold" alt="Stars"></a>
  <a href="https://github.com/DhanushNehru/GhostWalk/network/members"><img src="https://img.shields.io/github/forks/DhanushNehru/GhostWalk?style=flat-square" alt="Forks"></a>
  <a href="https://github.com/DhanushNehru/GhostWalk/issues"><img src="https://img.shields.io/github/issues/DhanushNehru/GhostWalk?style=flat-square" alt="Issues"></a>
  <a href="https://github.com/DhanushNehru/GhostWalk/blob/main/LICENSE"><img src="https://img.shields.io/github/license/DhanushNehru/GhostWalk?style=flat-square" alt="License"></a>
  <a href="https://github.com/DhanushNehru/GhostWalk"><img src="https://img.shields.io/github/languages/top/DhanushNehru/GhostWalk?style=flat-square" alt="Top language"></a>
</p>

---

GhostWalk fetches a web page, parses it into a **flat arena DOM** (every node lives in one contiguous vector, referenced by a 4-byte handle), runs the page's inline JavaScript in a bounded engine, and dumps the result the way an agent wants it: the DOM tree, plain text, semantic Markdown, or JSON. No Chromium, no GPU.

> **Status: working MVP.** The fetch -> parse -> execute-inline-JS -> dump pipeline runs today (see the demo below). Style, layout, rendering, CDP and MCP are designed and roadmapped - see [ARCHITECTURE.md](ARCHITECTURE.md) and [ROADMAP.md](ROADMAP.md). Contributions welcome: check the [open issues](https://github.com/DhanushNehru/GhostWalk/issues).

## See it work

The repo ships a demo page whose inline scripts change the title and rewrite the footer. Run GhostWalk on it:

```console
$ cargo build --release
$ ./target/release/ghostwalk examples/demo.html --dump tree
```

```
#document
  <!DOCTYPE html>
  <html>
    <head>
      <title>
        "ghostwalk demo (modified by JS)"
      ...
      <footer>
        <p#status>
          "JS ran and mutated this DOM."
```

The title above is not the one in the HTML file. GhostWalk actually executed the page's JavaScript against the live DOM, and the dump shows the mutated tree, not the static markup. Run it with `--stats` and it reports about a millisecond of work end to end and 8.5 MiB peak RSS on a 2-core Linux sandbox - your numbers will vary, see [Measured](#measured-mvp-release-build) below.

Want Markdown instead of a tree?

```console
$ ./target/release/ghostwalk https://example.com --dump md
$ ./target/release/ghostwalk examples/demo.html --dump md --stats
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

- HTML5 parsing via [html5ever](https://github.com/servo/html5ever) (the Servo project's standards-track parser) into GhostWalk's own arena storage
- Live DOM bridge for JavaScript: `document.querySelector(All)`, `document.title`, `document.body`, element `textContent` get/set, `get/setAttribute` - JS mutations are visible in the dumps
- Bounded JS execution: the [Boa](https://github.com/boa-dev/boa) engine with a loop-iteration budget that stops runaway loops, recursion/stack limits, and no network or file access from page JS. This is resource bounding, **not** a security sandbox: a wall-clock timeout for page JS is still an [open issue](https://github.com/DhanushNehru/GhostWalk/issues/5), so don't point GhostWalk at pages you don't trust yet. Details in the security model in [ARCHITECTURE.md](ARCHITECTURE.md)
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

The project-level targets are sub-30 ms startup and sub-50 MiB idle RSS for the full engine (with rendering); the numbers above cover only the current MVP pipeline, not an idle browser process. These are single-machine measurements, not guarantees - the benchmark harness that tracks them across a real corpus is an [open issue](https://github.com/DhanushNehru/GhostWalk/issues/7).

## What GhostWalk is not (yet)

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

GhostWalk is early on purpose: the MVP is small, the design space is large, and good contributions shape it. See [CONTRIBUTING.md](CONTRIBUTING.md) for the ground rules.

Good places to start:

- [Expand selector support: combinators, attribute selectors, structural pseudo-classes](https://github.com/DhanushNehru/GhostWalk/issues/2) (good first issue)
- [Render proper Markdown tables and nested lists in the extractor](https://github.com/DhanushNehru/GhostWalk/issues/6) (good first issue)
- [Fuzz the TreeSink and arena DOM invariants](https://github.com/DhanushNehru/GhostWalk/issues/8) (good first issue)
- [Make NodeId generational so stale ids cannot alias recycled slots](https://github.com/DhanushNehru/GhostWalk/issues/11) (good first issue)

Bigger pieces the project actively needs: the [async networking actor](https://github.com/DhanushNehru/GhostWalk/issues/3), a [QuickJS-ng backend behind an engine trait](https://github.com/DhanushNehru/GhostWalk/issues/4), a [wall-clock JS timeout](https://github.com/DhanushNehru/GhostWalk/issues/5), the [benchmark harness](https://github.com/DhanushNehru/GhostWalk/issues/7), the [CDP subset](https://github.com/DhanushNehru/GhostWalk/issues/9), and the [MCP server](https://github.com/DhanushNehru/GhostWalk/issues/10).

## License

MIT - see [LICENSE](LICENSE).
