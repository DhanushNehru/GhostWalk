# Roadmap

M0 is what exists today. Everything after M0 is planned work; each item has
or will get a tracking issue. Order is value-first: agents get usable output
long before pixels exist.

## M0 - MVP pipeline (done)

- Flat arena DOM with html5ever tree-builder sink
- Selector subset (tag, `#id`, `.class`, descendant, lists)
- Text, Markdown, tree and JSON dumps
- Boa JS runtime with live DOM bridge and loop/recursion budgets
- Bounded HTTP(S) fetch (timeout, redirects, byte ceiling)
- CLI: `ghostwalk <url|file> --dump tree|text|md|json`

## M1 - Async core

- Tokio actor pipeline: net actor, streaming parser actor, JS actor
- Command-buffer DOM mutations (single-writer arena)
- Backpressure and per-stage ceilings

## M2 - JavaScript, round two

- Engine trait; QuickJS-ng backend via rquickjs
- Wall-clock cooperative timeout for page JS
- External `<script src>` loading under the fetch budgets; basic module support

## M3 - Style, layout, pixels

- Component stores (style/layout/paint) keyed by NodeId
- CSS cascade subset with selector index; Rayon-parallel matching
- Layout: block/inline/flex subset
- tiny-skia software rasterizer; PNG screenshots and element geometry

## M4 - CDP subset

- WebSocket server; Target, Page, Runtime, Network domains (documented subset)
- Playwright/Puppeteer interop tests against the subset

## M5 - MCP server

- `ghostwalk-mcp`: fetch, extract-text, extract-markdown, query, screenshot as MCP tools
- Per-call budgets and result size caps

## M6 - Fingerprint profiles

- TLS ClientHello shaping (rustls / BoringSSL)
- HTTP/2 and HTTP/3 frame ordering profiles
- Deterministic per-profile canvas/WebGL entropy (post-M3)

## M7 - Extensions

- wasmtime-based agent plugins (custom extractors/transformers) under budgets

## Ongoing

- Benchmark harness: startup, RSS, pipeline latency across a URL corpus
- Parser fuzzing (cargo-fuzz) and html5lib-tests fixtures
