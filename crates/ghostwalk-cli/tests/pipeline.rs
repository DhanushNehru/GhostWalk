//! End-to-end pipeline test: parse a page, run its inline JS, and
//! verify the mutation is visible in the dumped output.

use std::process::Command;

const DEMO_HTML: &str = r##"<!doctype html>
<html><head><title>before</title></head>
<body>
<h1 id="app">placeholder</h1>
<script>
  document.title = "after";
  document.querySelector("#app").textContent = "mutated by js";
  console.log("pipeline-ok");
</script>
</body></html>"##;

#[test]
fn fetch_parse_js_dump_pipeline() {
    let dir = std::env::temp_dir();
    let path = dir.join("ghostwalk-pipeline-test.html");
    std::fs::write(&path, DEMO_HTML).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ghostwalk"))
        .arg(&path)
        .arg("--dump")
        .arg("json")
        .output()
        .expect("failed to run ghostwalk");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""title": "after""#), "title mutation: {stdout}");
    assert!(
        stdout.contains("mutated by js"),
        "textContent mutation: {stdout}"
    );
    assert!(stdout.contains("pipeline-ok"), "console capture: {stdout}");

    // --no-js must show the un-mutated document.
    let output = Command::new(env!("CARGO_BIN_EXE_ghostwalk"))
        .arg(&path)
        .arg("--dump")
        .arg("text")
        .arg("--no-js")
        .output()
        .expect("failed to run ghostwalk");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("placeholder"), "no-js dump: {stdout}");

    std::fs::remove_file(&path).ok();
}
