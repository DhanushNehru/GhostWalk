//! ghostwalk CLI: fetch a URL (or read a file), parse it into the arena DOM,
//! run its inline scripts, and dump the result.
//!
//!   ghostwalk https://example.com --dump md
//!   ghostwalk page.html --dump tree --no-js
//!
//! This is the proof-of-concept pipeline from ARCHITECTURE.md:
//! fetch -> parse -> execute inline JS -> dump text/tree/markdown.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Instant;

use ghostwalk_dom::extract;
use ghostwalk_dom::parse::parse_html;
use ghostwalk_js::{collect_inline_scripts, JsConfig, JsRuntime};
use ghostwalk_net::{fetch, FetchOptions};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
ghostwalk - a data-oriented headless web engine (MVP)

USAGE:
    ghostwalk <URL | FILE> [OPTIONS]

ARGS:
    <URL | FILE>          http(s) URL to fetch, or a local HTML file

OPTIONS:
    --dump <FORMAT>       tree (default) | text | md | json
    --no-js               skip inline <script> execution
    --js-loop-limit <N>   max JS loop iterations per engine (default 5000000)
    --timeout-ms <N>      total fetch timeout (default 10000)
    --max-bytes <N>       response body ceiling (default 5242880)
    --user-agent <UA>     User-Agent header
    --stats               print stage timings and peak RSS to stderr
    --version             print version
    -h, --help            print this help
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dump {
    Tree,
    Text,
    Markdown,
    Json,
}

struct Args {
    source: String,
    dump: Dump,
    run_js: bool,
    js_loop_limit: u64,
    fetch: FetchOptions,
    stats: bool,
}

fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut source: Option<String> = None;
    let mut dump = Dump::Tree;
    let mut run_js = true;
    let mut js_loop_limit = JsConfig::default().loop_iteration_limit;
    let mut fetch = FetchOptions::default();
    let mut stats = false;

    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg {
            "-h" | "--help" => return Ok(None),
            "--version" => return Ok(None),
            "--dump" => {
                let v = value(&mut i)?;
                dump = match v.as_str() {
                    "tree" => Dump::Tree,
                    "text" => Dump::Text,
                    "md" | "markdown" => Dump::Markdown,
                    "json" => Dump::Json,
                    other => return Err(format!("unknown dump format: {other}")),
                };
            }
            "--no-js" => run_js = false,
            "--js-loop-limit" => {
                js_loop_limit = value(&mut i)?
                    .parse()
                    .map_err(|_| "invalid --js-loop-limit".to_string())?;
            }
            "--timeout-ms" => {
                let ms: u64 = value(&mut i)?
                    .parse()
                    .map_err(|_| "invalid --timeout-ms".to_string())?;
                fetch.timeout = std::time::Duration::from_millis(ms);
            }
            "--max-bytes" => {
                fetch.max_bytes = value(&mut i)?
                    .parse()
                    .map_err(|_| "invalid --max-bytes".to_string())?;
            }
            "--user-agent" => fetch.user_agent = value(&mut i)?,
            "--stats" => stats = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option: {other}"));
            }
            other => {
                if source.is_some() {
                    return Err(format!("unexpected argument: {other}"));
                }
                source = Some(other.to_string());
            }
        }
        i += 1;
    }

    match source {
        Some(source) => Ok(Some(Args {
            source,
            dump,
            run_js,
            js_loop_limit,
            fetch,
            stats,
        })),
        None => Ok(None),
    }
}

fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().ok();
        }
    }
    None
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--version") {
        println!("ghostwalk {VERSION}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_args(&argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let t_start = Instant::now();

    // 1. Fetch (or read a file).
    let (body, origin): (String, String) = if args.source.starts_with("http://")
        || args.source.starts_with("https://")
    {
        match fetch(&args.source, &args.fetch) {
            Ok(r) => (r.body, r.final_url),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match std::fs::read_to_string(&args.source) {
            Ok(s) => (s, args.source.clone()),
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", args.source);
                return ExitCode::FAILURE;
            }
        }
    };
    let t_fetch = Instant::now();

    // 2. Parse into the arena DOM (once).
    let dom = Rc::new(RefCell::new(parse_html(&body)));
    let scripts = if args.run_js {
        collect_inline_scripts(&dom.borrow())
    } else {
        Vec::new()
    };
    let t_parse = Instant::now();

    // 3. Execute inline scripts against the live DOM.
    let mut console = Vec::new();
    let mut scripts_run = 0usize;
    let mut script_errors: Vec<String> = Vec::new();
    if args.run_js {
        let config = JsConfig {
            loop_iteration_limit: args.js_loop_limit,
            ..JsConfig::default()
        };
        match JsRuntime::new(dom.clone(), &config) {
            Ok(mut rt) => {
                for (idx, source) in scripts.iter().enumerate() {
                    match rt.run_script(source) {
                        Ok(_) => scripts_run += 1,
                        Err(e) => script_errors.push(format!("script {}: {e}", idx + 1)),
                    }
                }
                console = rt.take_console_output();
            }
            Err(e) => {
                script_errors.push(format!("engine init: {e}"));
            }
        }
    }
    let t_js = Instant::now();

    // 4. Reclaim detached nodes after scripts release their handles, then dump.
    dom.borrow_mut().compact();
    {
        let dom = dom.borrow();
        match args.dump {
            Dump::Tree => print!("{}", extract::dump_tree(&dom)),
            Dump::Text => println!("{}", extract::extract_text(&dom)),
            Dump::Markdown => println!("{}", extract::extract_markdown(&dom)),
            Dump::Json => {
                let title = dom
                    .find_first_element_by_tag("title")
                    .map(|id| dom.text_content(id))
                    .unwrap_or_default();
                let text = extract::extract_text(&dom);
                let console_json: Vec<String> = console
                    .iter()
                    .map(|m| {
                        format!(
                            "{{\"level\":\"{}\",\"message\":\"{}\"}}",
                            json_escape(&m.level),
                            json_escape(&m.message)
                        )
                    })
                    .collect();
                let errors_json: Vec<String> = script_errors
                    .iter()
                    .map(|e| format!("\"{}\"", json_escape(e)))
                    .collect();
                println!(
                    "{{\n  \"url\": \"{}\",\n  \"title\": \"{}\",\n  \"scripts_run\": {},\n  \"script_errors\": [{}],\n  \"console\": [{}],\n  \"text\": \"{}\"\n}}",
                    json_escape(&origin),
                    json_escape(&title),
                    scripts_run,
                    errors_json.join(", "),
                    console_json.join(", "),
                    json_escape(&text),
                );
            }
        }
    }

    for e in &script_errors {
        eprintln!("warning: {e}");
    }

    if args.stats {
        let t_dump = Instant::now();
        eprintln!("stats:");
        eprintln!("  fetch+read : {:>7.2?}", t_fetch - t_start);
        eprintln!("  parse      : {:>7.2?}", t_parse - t_fetch);
        eprintln!("  js         : {:>7.2?}", t_js - t_parse);
        eprintln!("  dump       : {:>7.2?}", t_dump - t_js);
        eprintln!("  total      : {:>7.2?}", t_dump - t_start);
        if let Some(kb) = peak_rss_kb() {
            eprintln!("  peak RSS   : {:.1} MiB", kb as f64 / 1024.0);
        }
        eprintln!("  dom nodes  : {}", dom.borrow().node_count());
    }

    ExitCode::SUCCESS
}

