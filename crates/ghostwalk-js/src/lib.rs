//! # ghostwalk-js
//!
//! JavaScript runtime and DOM bridge for the ghostwalk engine.
//!
//! MVP engine: [Boa](https://github.com/boa-dev/boa), a pure-Rust
//! ECMAScript engine (>90% test262 conformance), chosen so the whole
//! engine builds with `cargo build` alone - no C++ toolchain, no
//! vendored blobs. The engine is intentionally small here; the bridge
//! is the interesting part and the engine is swappable behind this
//! module (QuickJS-ng via rquickjs is the planned second backend).
//!
//! ## The bridge
//!
//! JS gets a `document` object backed live by the Rust arena: queries
//! run the ghostwalk-dom selector engine, and element handles read and
//! mutate the arena in place (no DOM serialization round-trips).
//! Mutations are visible to the text/markdown dumpers afterwards.
//!
//! ## Hostile content
//!
//! Untrusted pages get bounded execution: a loop-iteration limit (the
//! main runaway-script guard) plus Boa's recursion/stack limits. There
//! is no network, no file access, and no `import` from inside page JS.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::builtins::JsArray;
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
    Source,
};
use ghostwalk_dom::{Dom, NodeId};

/// One captured `console.*` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleMessage {
    pub level: String,
    pub message: String,
}

thread_local! {
    /// Console sink for the current thread. The CLI is single-threaded;
    /// the actor pipeline (roadmap) will route this through a channel.
    static CONSOLE_LOG: RefCell<Vec<ConsoleMessage>> = const { RefCell::new(Vec::new()) };
}

/// Execution limits applied to every script.
#[derive(Debug, Clone)]
pub struct JsConfig {
    /// Total loop iterations before the engine throws. Default: 5M.
    pub loop_iteration_limit: u64,
    /// Call recursion depth. Default: Boa's built-in limit.
    pub recursion_limit: Option<usize>,
}

impl Default for JsConfig {
    fn default() -> Self {
        JsConfig {
            loop_iteration_limit: 5_000_000,
            recursion_limit: None,
        }
    }
}

/// A JS runtime bound to one document.
pub struct JsRuntime {
    context: Context,
}

impl JsRuntime {
    /// Create a runtime with `console`, `document` and `window` bound
    /// to the given arena DOM.
    pub fn new(dom: Rc<RefCell<Dom>>, config: &JsConfig) -> Result<Self, JsError> {
        let mut context = Context::default();
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(config.loop_iteration_limit);
        if let Some(limit) = config.recursion_limit {
            context.runtime_limits_mut().set_recursion_limit(limit);
        }
        install_console(&mut context)?;
        install_document(&mut context, dom)?;
        Ok(JsRuntime { context })
    }

    /// Evaluate one script. State persists across calls, like a page.
    pub fn run_script(&mut self, source: &str) -> JsResult<JsValue> {
        self.context.eval(Source::from_bytes(source))
    }

    /// Drain captured `console.*` output.
    pub fn take_console_output(&self) -> Vec<ConsoleMessage> {
        CONSOLE_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }
}

/// Source texts of the inline `<script>` elements, in document order.
///
/// Scripts with `src`, a non-JS `type`, or `type="module"` are skipped
/// (external loading and module graphs are roadmap items).
pub fn collect_inline_scripts(dom: &Dom) -> Vec<String> {
    let mut out = Vec::new();
    for id in dom.descendants(dom.root()) {
        if dom.tag_name(id) != Some("script") {
            continue;
        }
        if dom.attr(id, "src").is_some() {
            continue;
        }
        let runnable = match dom.attr(id, "type") {
            None => true,
            Some(t) => {
                let t = t.trim();
                t.is_empty()
                    || t.eq_ignore_ascii_case("text/javascript")
                    || t.eq_ignore_ascii_case("application/javascript")
            }
        };
        if runnable {
            out.push(dom.text_content(id));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

fn console_fn(level: &'static str) -> NativeFunction {
    // SAFETY: the closure captures no GC-managed values, only a
    // &'static str; nothing it touches needs tracing.
    unsafe {
        NativeFunction::from_closure(move |_this, args, ctx| {
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(arg.to_string(ctx)?.to_std_string_escaped());
            }
            CONSOLE_LOG.with(|log| {
                log.borrow_mut().push(ConsoleMessage {
                    level: level.to_string(),
                    message: parts.join(" "),
                });
            });
            Ok(JsValue::undefined())
        })
    }
}

fn install_console(ctx: &mut Context) -> Result<(), JsError> {
    let console = ObjectInitializer::new(ctx)
        .function(console_fn("log"), js_string!("log"), 0)
        .function(console_fn("info"), js_string!("info"), 0)
        .function(console_fn("warn"), js_string!("warn"), 0)
        .function(console_fn("error"), js_string!("error"), 0)
        .build();
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// document
// ---------------------------------------------------------------------------

fn selector_arg(args: &[JsValue], ctx: &mut Context) -> JsResult<String> {
    match args.first() {
        Some(v) => Ok(v.to_string(ctx)?.to_std_string_escaped()),
        None => Ok(String::new()),
    }
}

fn syntax_error(selector: &str) -> JsError {
    JsNativeError::syntax()
        .with_message(format!("'{selector}' is not a valid selector"))
        .into()
}

fn install_document(ctx: &mut Context, dom: Rc<RefCell<Dom>>) -> Result<(), JsError> {
    let realm = ctx.realm().clone();

    // document.querySelector(selector)
    let query_one = {
        let dom = dom.clone();
        // SAFETY: captures hold only Rc<RefCell<Dom>> (plain Rust data,
        // no GC-managed JsValues), so nothing needs tracing.
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let sel = selector_arg(args, ctx)?;
                match ghostwalk_dom::select::query_selector(&dom.borrow(), &sel) {
                    Ok(Some(id)) => Ok(element_object(ctx, dom.clone(), id)),
                    Ok(None) => Ok(JsValue::null()),
                    Err(_) => Err(syntax_error(&sel)),
                }
            })
        }
    };

    // document.querySelectorAll(selector)
    let query_all = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let sel = selector_arg(args, ctx)?;
                match ghostwalk_dom::select::query_selector_all(&dom.borrow(), &sel) {
                    Ok(ids) => {
                        let arr = JsArray::new(ctx)?;
                        for id in ids {
                            arr.push(element_object(ctx, dom.clone(), id), ctx)?;
                        }
                        Ok(arr.into())
                    }
                    Err(_) => Err(syntax_error(&sel)),
                }
            })
        }
    };

    // document.title getter
    let title_get = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let dom = dom.borrow();
                let title = dom
                    .find_first_element_by_tag("title")
                    .map(|id| dom.text_content(id))
                    .unwrap_or_default();
                Ok(JsValue::from(JsString::from(title)))
            })
        }
    };

    // document.title setter
    let title_set = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let value = selector_arg(args, ctx)?;
                let mut dom = dom.borrow_mut();
                let title_id = match dom.find_first_element_by_tag("title") {
                    Some(id) => id,
                    None => {
                        // Create <title> under <head> if the page lacks one.
                        let t = dom.create_html_element("title");
                        match dom.find_first_element_by_tag("head") {
                            Some(head) => dom.append(head, t),
                            None => {
                                let root = dom.root();
                                dom.append(root, t);
                            }
                        }
                        t
                    }
                };
                dom.set_text_content(title_id, &value);
                Ok(JsValue::undefined())
            })
        }
    };

    // document.body getter
    let body_get = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, _args, ctx| {
                match dom.borrow().find_first_element_by_tag("body") {
                    Some(id) => Ok(element_object(ctx, dom.clone(), id)),
                    None => Ok(JsValue::null()),
                }
            })
        }
    };

    let title_get = FunctionObjectBuilder::new(&realm, title_get)
        .name(js_string!("get title"))
        .length(0)
        .constructor(false)
        .build();
    let title_set = FunctionObjectBuilder::new(&realm, title_set)
        .name(js_string!("set title"))
        .length(1)
        .constructor(false)
        .build();
    let body_get = FunctionObjectBuilder::new(&realm, body_get)
        .name(js_string!("get body"))
        .length(0)
        .constructor(false)
        .build();

    let flags = Attribute::ENUMERABLE | Attribute::CONFIGURABLE;
    let document = ObjectInitializer::new(ctx)
        .function(query_one, js_string!("querySelector"), 1)
        .function(query_all, js_string!("querySelectorAll"), 1)
        .accessor(js_string!("title"), Some(title_get), Some(title_set), flags)
        .accessor(js_string!("body"), Some(body_get), None, flags)
        .build();

    ctx.register_global_property(js_string!("document"), document, Attribute::all())?;

    // window as an alias for globalThis, the way pages expect.
    let global = ctx.global_object();
    ctx.register_global_property(js_string!("window"), global, Attribute::all())?;

    Ok(())
}

// ---------------------------------------------------------------------------
// element handles
// ---------------------------------------------------------------------------

/// A live handle to one arena node, exposed to JS as a plain object.
/// Reads and writes go straight to the Rust arena through the captured
/// `Rc<RefCell<Dom>>`.
fn element_object(ctx: &mut Context, dom: Rc<RefCell<Dom>>, id: NodeId) -> JsValue {
    let realm = ctx.realm().clone();
    let tag = dom
        .borrow()
        .tag_name(id)
        .unwrap_or("")
        .to_uppercase();

    let text_get = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, _args, _ctx| {
                let text = dom.borrow().text_content(id);
                Ok(JsValue::from(JsString::from(text)))
            })
        }
    };
    let text_set = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let value = selector_arg(args, ctx)?;
                dom.borrow_mut().set_text_content(id, &value);
                Ok(JsValue::undefined())
            })
        }
    };
    let get_attr = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let name = selector_arg(args, ctx)?;
                match dom.borrow().attr(id, &name) {
                    Some(v) => Ok(JsValue::from(JsString::from(v))),
                    None => Ok(JsValue::null()),
                }
            })
        }
    };
    let set_attr = {
        let dom = dom.clone();
        unsafe {
            NativeFunction::from_closure(move |_this, args, ctx| {
                let name = selector_arg(args, ctx)?;
                let value = args
                    .get(1)
                    .map(|v| v.to_string(ctx))
                    .transpose()?
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default();
                dom.borrow_mut().set_attr(id, &name, &value);
                Ok(JsValue::undefined())
            })
        }
    };

    let text_get = FunctionObjectBuilder::new(&realm, text_get)
        .name(js_string!("get textContent"))
        .length(0)
        .constructor(false)
        .build();
    let text_set = FunctionObjectBuilder::new(&realm, text_set)
        .name(js_string!("set textContent"))
        .length(1)
        .constructor(false)
        .build();

    let flags = Attribute::ENUMERABLE | Attribute::CONFIGURABLE;
    ObjectInitializer::new(ctx)
        .property(
            js_string!("tagName"),
            JsString::from(tag),
            Attribute::READONLY | Attribute::ENUMERABLE,
        )
        .accessor(js_string!("textContent"), Some(text_get), Some(text_set), flags)
        .function(get_attr, js_string!("getAttribute"), 1)
        .function(set_attr, js_string!("setAttribute"), 2)
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostwalk_dom::parse::parse_html;

    fn runtime(html: &str) -> (JsRuntime, Rc<RefCell<Dom>>) {
        let dom = Rc::new(RefCell::new(parse_html(html)));
        let rt = JsRuntime::new(dom.clone(), &JsConfig::default()).unwrap();
        (rt, dom)
    }

    #[test]
    fn script_reads_and_mutates_dom() {
        let (mut rt, dom) = runtime(
            r#"<!doctype html><html><head><title>old</title></head>
               <body><h1 id="app">before</h1></body></html>"#,
        );
        rt.run_script(r##"document.querySelector("#app").textContent = "after";"##)
            .unwrap();
        rt.run_script(r#"document.title = "new title";"#).unwrap();
        assert_eq!(rt.run_script("document.title").unwrap().to_string(&mut rt.context).unwrap().to_std_string_escaped(), "new title");
        let app = dom.borrow().find_first_element_by_tag("h1").unwrap();
        assert_eq!(dom.borrow().text_content(app), "after");
    }

    #[test]
    fn console_is_captured() {
        let (mut rt, _dom) = runtime("<html><body></body></html>");
        rt.run_script(r#"console.log("hello", 1 + 1);"#).unwrap();
        let out = rt.take_console_output();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].message, "hello 2");
    }

    #[test]
    fn runaway_loops_are_bounded() {
        let (mut rt, _dom) = runtime("<html><body></body></html>");
        let result = rt.run_script("while (true) {}");
        assert!(result.is_err());
    }

    #[test]
    fn query_selector_all_returns_array() {
        let (mut rt, _dom) = runtime("<html><body><p>a</p><p>b</p></body></html>");
        let v = rt
            .run_script("document.querySelectorAll('p').length")
            .unwrap();
        assert_eq!(v.to_string(&mut rt.context).unwrap().to_std_string_escaped(), "2");
    }
}
