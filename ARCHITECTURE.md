# ghostwalk architecture

Design blueprint for a data-oriented headless web engine in Rust, built for
AI agents, scraping, and automation. Each section marks what is
**implemented** in the MVP versus **planned**, and measurements are labeled
with their method. Landscape reference: [Obscura](https://github.com/h4ckf0r0day/obscura)
(Rust headless browser for AI agents, V8 + CDP) and
[Servo](https://github.com/servo/servo) are the closest neighbors; ghostwalk's
distinct bet is a data-oriented core with a swappable, lightweight JS engine.

---

## 1. Engine architecture: arenas and actors, not trees and pointers

### 1.1 The flat arena DOM (implemented: `ghostwalk-dom`)

Classic browser DOMs are webs of owning pointers (`Rc<Node>`, weak parents,
RefCell interiors). In Rust that fights the borrow checker, scatters memory,
and makes parallel style/layout painful.

GhostWalk stores every node in one contiguous `Vec<Node>` and refers to nodes by a
4-byte `Copy` handle:

```rust
pub struct NodeId(NonZeroU32);          // index + 1, niche-optimized

pub struct Node {
    pub parent:       Option<NodeId>,   // each Option<NodeId> is 4 bytes
    pub prev_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub first_child:  Option<NodeId>,
    pub last_child:   Option<NodeId>,
    pub data: NodeData,                 // Document | Doctype | Element | Text | Comment | PI
}

pub struct Dom {
    nodes: Vec<Node>,                   // the whole tree, one allocation stream
    root: NodeId,
    quirks_mode: QuirksMode,
    parse_error_count: u64,
}
```

Consequences:

- **No borrow fights.** Mutation is `&mut Dom` plus indices; no `Rc` cycles,
  no `RefCell` panics, no lifetimes in user code.
- **Cache-friendly.** Nodes are allocated in parse order (roughly document
  order), so traversals chase adjacent memory, not pointers across the heap.
- **Cheap handles.** `NodeId` is `Copy`, hashable, and safe to store in
  component tables - the property an ECS-style design needs.
- **Parsing is decoupled from storage.** The HTML5 tree builder is
  [html5ever](https://github.com/servo/html5ever) (the Servo project's
  standards-track parser); ghostwalk implements its `TreeSink` trait and turns
  tree-builder callbacks into arena operations (`crates/ghostwalk-dom/src/parse.rs`).

### 1.2 From arena to ECS (planned)

The arena is the bridge to a full data-oriented engine. The plan:

- **Components as parallel stores.** Computed styles, layout boxes, and paint
  fragments each live in their own contiguous store keyed by `NodeId`
  (a sparse-set per component type), not as fields hung off DOM nodes.
  Systems iterate dense component arrays; entities stay tiny ids.
- **Structure-of-arrays nodes.** Split `Node` into parallel arrays
  (`parents: Vec<u32>`, `first_child: Vec<u32>`, ..., `data: Vec<NodeData>`)
  so traversal systems touch only the arrays they need.
- **Generational ids.** `NodeId` gains a generation so stale handles are
  detected instead of silently aliasing a recycled slot; arena compaction
  reclaims detached garbage (the MVP's `set_text_content` leaves orphaned
  slots by design - see `node.rs`).
- **Style as data.** A flat rule table with a precomputed selector index;
  matching writes computed-style components. Invalidation is dirty flags on
  `NodeId`s, not observer callbacks.

### 1.3 Event-driven actor pipeline (planned; MVP is synchronous)

Target concurrency model, per stage:

```
            tokio mpsc channels (backpressured)
 NetActor ──────────► ParserActor ──────────► DOM/Core ──► JS actor
 (ureq/hyper,        (stream chunks into      (owns the    (owns the engine,
  h2/h3 later)        html5ever TreeSink)      arena)       runs scripts)
                              │
                              ▼ rayon par_iter
                     Style + Layout systems
                     (data-parallel over
                      component stores)
```

- **Tokio** for I/O actors and the event pump; **Rayon** for CPU-parallel
  style/layout systems over the component stores. Actors communicate with
  messages; the JS actor receives a command buffer of DOM mutations and
  returns query results, so the arena has exactly one writer at a time.
- **Backpressure and ceilings at every hop**: byte ceilings (implemented in
  `ghostwalk-net`), parse chunk caps, JS execution budgets (implemented),
  layout time slices.
- The MVP deliberately keeps this single-threaded and synchronous; the trait
  surfaces (`FetchOptions`/`fetch`, `TreeSink`, `JsRuntime`) are the seams
  the actors will hang on.

---

## 2. JS/Wasm runtime strategy

### 2.1 Engine comparison (sources: project READMEs/docs as of Nov 2026)

| Engine | Rust binding | Conformance | Footprint/build | Notes |
| --- | --- | --- | --- | --- |
| **V8** | [`rusty_v8`](https://github.com/denoland/rusty_v8) | Full, JIT | ~40 MB static in a binary ([v8x measurements](https://denoland.github.io/v8x/)); V8 builds from source, historically ~30 min | Maximum compatibility, maximum weight. What Obscura uses. |
| **V8 via deno_core** | `deno_core` | Same | Same V8 cost + op/extension machinery | Great for building a runtime, not a browser core. |
| **JavaScriptCore** | `javascriptcore-rs` / [v8x `jsc`](https://denoland.github.io/v8x/) | Full, JIT | ~48 MB vendored static; 0 extra with the macOS system framework | Attractive on Apple platforms only. |
| **SpiderMonkey** | [`mozjs`](https://github.com/servo/mozjs) | Full, JIT | Large; offers prebuilt archives (tracks mozilla-esr153) | Battle-tested in Servo; heavyweight build/bindgen. |
| **QuickJS-ng** | [`rquickjs`](https://github.com/DelSkayn/rquickjs) | ES2023, no JIT | **~1 MB static** (v8x measurements); quickjs-ng base since rquickjs 0.9 | The sweet spot for footprint; actively maintained. |
| **Boa** | [`boa_engine`](https://github.com/boa-dev/boa) | >90% of ECMAScript (test262), interpreter | Pure Rust, `cargo build` only - no C/C++ toolchain | Youngest; conformance still climbing. |

### 2.2 ghostwalk's choice (implemented)

**MVP: Boa** (`ghostwalk-js`), because the entire engine then builds with a bare
`cargo build` - the lowest possible contributor barrier, and zero vendored
binary blobs. **Planned second backend: QuickJS-ng via rquickjs**, behind an
engine trait so `ghostwalk-js` callers never see the difference. V8 remains an
option behind the same trait for a "compat" build profile, not the default.

### 2.3 The DOM-to-JS bridge

Implemented (MVP): a **live handle bridge**. JS gets a `document` object;
queries run the Rust selector engine and return element handles whose
getters/setters close over an `Rc<RefCell<Dom>>` and read/mutate the arena in
place. No DOM serialization crosses the FFI boundary on either direction.

Planned hardening (this is the interesting engineering):

- **Snapshot reads, command-buffer writes.** Instead of shared mutable
  access, the JS actor receives an immutable snapshot for queries and emits
  mutation commands (`SetText(NodeId, String)`, ...) that the core applies
  between turns. One writer, deterministic ordering, replayable logs.
- **Native accessors, not JSON.** Property access stays native engine calls
  (Boa `NativeFunction` closures today); the design rule is *no JSON/string
  serialization on the hot path* - that is where embedding projects usually
  lose their performance.
- **Fuel + wall-clock budgets.** The MVP enforces a loop-iteration budget
  (Boa `RuntimeLimits`) plus recursion/stack limits; a cooperative
  wall-clock interrupt is a tracked issue.

### 2.4 Wasm (planned)

A `wasmtime`-backed extension point for agent-side plugins (custom
extractors, transformers) running under the same budgets. Not on the page's
critical path for the MVP.

---

## 3. Differentiators

### 3.1 Anti-fingerprinting by design (planned; nothing here is implemented)

Roadmapped transport and rendering levers, each behind an explicit profile:

- **TLS ClientHello shaping** - rustls with custom extension ordering, or
  BoringSSL via the `boring` crate, to present a chosen ClientHello.
- **HTTP/2 and HTTP/3 frame shaping** - SETTINGS/HEADERS ordering and
  priority patterns (h2/h3 + quinn), matching a stated profile.
- **Canvas/WebGL entropy control** - deterministic per-profile noise in the
  tiny-skia rasterizer output and GL parameter normalization, so fingerprint
  surfaces are stable per profile instead of unique per machine.

Scope note: this exists so operators can run *their own* automation with
stable, declared profiles. ghostwalk does not ship CAPTCHA bypass, credential
abuse, or paywall evasion.

### 3.2 Agent-first tooling

- **Semantic Markdown straight from the DOM (implemented).**
  `ghostwalk <url> --dump md` emits headings, links, lists, emphasis, code and
  blockquotes without a readability sidecar or a second parse.
- **Structured dumps (implemented).** Tree, text, and JSON (title, console
  capture, script errors, extracted text).
- **MCP server (planned).** A `ghostwalk-mcp` crate speaking the
  [Model Context Protocol](https://modelcontextprotocol.org/) (spec
  2026-07-28), exposing fetch/extract as MCP tools so agents can call ghostwalk
  natively.
- **CDP compatibility (planned, honestly scoped).** A documented **subset**
  of the Chrome DevTools Protocol (Target/Page/Runtime/Network domains) as a
  WebSocket server. Target: drop-in for common Playwright/Puppeteer flows.
  It will not be full Chromium parity; the subset and its limits will be
  documented per domain.

### 3.3 Resource ceiling

- **Project targets** (full engine): sub-30 ms startup, sub-50 MiB idle RSS.
- **MVP measurements** (single 2-core Linux sandbox, best of 3): process
  start-to-exit ~1-2 ms (wall time of `ghostwalk --version`); full local
  pipeline ~1 ms; peak RSS during that run ~8.5 MiB (`VmHWM`); stripped binary ~12 MB.
  These describe today's pipeline only - not an idle browser - and move to a
  tracked benchmark harness (open issue).
- **Rendering (planned):** software rasterization with
  [tiny-skia](https://github.com/RazrFalcon/tiny-skia) (a compact Skia
  subset) for screenshots and geometry - never full Skia/Chromium.

---

## 4. Security model for hostile content

The MVP provides **resource bounding, not a security sandbox**:

- Implemented: per-request timeouts, redirect caps, response byte ceilings;
  JS loop-iteration budget, recursion and stack limits; page JS has no
  network, no filesystem, no `import`; external `<script src>` is not loaded.
- Not yet: OS-level isolation. Planned: process-per-site with
  seccomp/namespace restrictions, an IPC boundary between the JS actor and
  the network actor, and capability-based resource grants. Until then, run
  ghostwalk against content you already trust, inside your own container/VM
  boundary.

---

## 5. Crate map and proof-of-concept pipeline

| Crate | Status | Role |
| --- | --- | --- |
| `ghostwalk-dom` | implemented | Arena DOM, html5ever `TreeSink`, selector subset, text/Markdown/tree dumps |
| `ghostwalk-net` | implemented | Bounded blocking HTTP(S) fetch behind `FetchOptions` |
| `ghostwalk-js` | implemented | Boa runtime, live `document` bridge, console capture, budgets |
| `ghostwalk-cli` | implemented | `ghostwalk` binary: the PoC pipeline |
| `ghostwalk-actor` | planned | Tokio actor pipeline (M1) |
| `ghostwalk-style` / `ghostwalk-layout` / `ghostwalk-paint` | planned | Component stores, cascade, layout, tiny-skia raster (M3) |
| `ghostwalk-cdp` | planned | CDP subset WebSocket server (M4) |
| `ghostwalk-mcp` | planned | MCP server (M5) |

The PoC pipeline - **fetch URL -> parse HTML -> execute inline JS -> dump
parsed text/tree** - is the CLI itself:

```console
$ ghostwalk https://example.com --dump tree     # structural dump
$ ghostwalk page.html --dump md --stats         # semantic Markdown + timings
$ cargo test                               # unit + end-to-end pipeline tests
```

See [ROADMAP.md](ROADMAP.md) for milestone detail and
[CONTRIBUTING.md](CONTRIBUTING.md) to jump in.
