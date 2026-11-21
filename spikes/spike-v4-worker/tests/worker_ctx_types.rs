//! The ctx types an extension author gets (`workers/ctx.d.ts`, `workers/aip_ctx.py`) against the real workers.
//! - Type side: a typed extension passes the checker and a file of wrong calls is refused on exactly the lines marked `ERR`
//!   (so a check that accepted everything would fail here). `tsc` is required (`AIP_TSC`, default `../spike-0-ts/node_modules/.bin/tsc`);
//!   `mypy` is checked when `AIP_MYPY` names it and reported as not run otherwise.
//! - Runtime side: the same typed extensions run in `worker.mjs` and `worker.py`; the calls they send and the values they get back
//!   are what the declarations say, so a declaration cannot drift from the worker unnoticed.
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf, process::Command, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command as AsyncCommand,
};

fn manifest(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn tsc_path() -> PathBuf {
    std::env::var_os("AIP_TSC").map(PathBuf::from).unwrap_or_else(|| manifest("../spike-0-ts/node_modules/.bin/tsc"))
}

fn check_js(file: &str) -> (bool, String) {
    let tsc = tsc_path();
    let out = Command::new(&tsc)
        .args(["--noEmit", "--strict", "--allowJs", "--checkJs", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext"])
        .args(["--skipLibCheck", "--lib", "esnext,dom"])
        .arg(manifest(file))
        .output()
        .unwrap_or_else(|error| panic!("tsc ({}) did not start: {error}; set AIP_TSC", tsc.display()));
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
}

/// 1-based numbers of the lines of `file` that carry `marker`.
fn marked_lines(file: &str, marker: &str) -> BTreeSet<usize> {
    std::fs::read_to_string(manifest(file)).unwrap().lines().enumerate().filter(|(_, line)| line.contains(marker)).map(|(i, _)| i + 1).collect()
}

/// Lines a checker reported as `<file>(<line>,<col>): error` (tsc) or `<file>:<line>: error` (mypy).
fn error_lines(output: &str, file_name: &str) -> BTreeSet<usize> {
    output
        .lines()
        .filter(|line| line.contains("error"))
        .filter_map(|line| {
            let rest = line.split(file_name).nth(1)?;
            let rest = rest.strip_prefix('(').or_else(|| rest.strip_prefix(':'))?;
            rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        })
        .collect()
}

#[test]
fn typed_extensions_pass_tsc_and_wrong_calls_are_refused_on_the_marked_lines() {
    for file in ["tests/fixtures/ctx-types/typed.mjs", "tests/fixtures/expr-ext-create/ex.mjs"] {
        let (ok, output) = check_js(file);
        assert!(ok, "{file} must type-check against workers/ctx.d.ts:\n{output}");
    }
    let neg = "tests/fixtures/ctx-types/neg.mjs";
    let (ok, output) = check_js(neg);
    assert!(!ok, "{neg} has wrong calls and must fail to type-check");
    let expected = marked_lines(neg, "// ERR");
    assert!(expected.len() >= 15, "the negative file marks its wrong lines: {expected:?}");
    assert_eq!(error_lines(&output, "neg.mjs"), expected, "tsc refused other lines than the marked ones:\n{output}");
}

#[test]
fn typed_python_extensions_pass_mypy_and_wrong_calls_are_refused_on_the_marked_lines() {
    let Some(mypy) = std::env::var_os("AIP_MYPY") else {
        eprintln!("미실행: AIP_MYPY가 없음(mypy 경로). Python Protocol 검사는 건너뜀");
        return;
    };
    let run = |file: &str| {
        let out = Command::new(&mypy)
            .env("MYPYPATH", manifest("workers"))
            .args(["--strict", "--no-incremental", "--cache-dir", "/dev/null"])
            .arg(manifest(file))
            .output()
            .unwrap_or_else(|error| panic!("mypy did not start: {error}"));
        (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string())
    };
    let (ok, output) = run("tests/fixtures/ctx-types/typed.py");
    assert!(ok, "typed.py must type-check against workers/aip_ctx.py:\n{output}");
    let neg = "tests/fixtures/ctx-types/neg.py";
    let (ok, output) = run(neg);
    assert!(!ok, "{neg} has wrong calls and must fail to type-check");
    let expected = marked_lines(neg, "# ERR");
    assert!(expected.len() >= 10, "the negative file marks its wrong lines: {expected:?}");
    assert_eq!(error_lines(&output, "neg.py"), expected, "mypy refused other lines than the marked ones:\n{output}");
}

/// Plays the server for one invoke: records every call the extension sends and answers it with the canned value of its operation.
async fn drive(lang: &str, implementation: &str, input: Value) -> (Vec<Value>, Value) {
    let dir = manifest("tests/fixtures/ctx-types");
    let mut command = match lang {
        "node" => {
            let mut command = AsyncCommand::new("node");
            command.args(["--input-type=module", "-e", include_str!("../workers/worker.mjs"), "--", dir.to_str().unwrap()]);
            command
        }
        _ => {
            let mut command = AsyncCommand::new("python3");
            command.args(["-P", "-c", include_str!("../workers/worker.py"), dir.to_str().unwrap()]);
            command
        }
    };
    let mut child =
        command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true).spawn().expect("worker runtime");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let invoke = json!({"type": "invoke", "invoke": 1, "impl": implementation, "input": input, "token": "t"});
    stdin.write_all(format!("{invoke}\n").as_bytes()).await.unwrap();
    let mut calls = vec![];
    let done = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("the worker ended before it was done");
            let message: Value = serde_json::from_str(&line).unwrap();
            match message["type"].as_str() {
                Some("call") => {
                    let value = match (message["op"].as_str().unwrap(), message["input"]["command"].as_str()) {
                        ("aggregate", _) => json!({"total": 5}),
                        ("read", _) => json!({"rows": [{"id": "10"}]}),
                        ("apply", _) => json!({"changed": ["10"], "unchanged": ["11"]}),
                        ("create", _) => json!({"created": [{"resource": "Todo", "id": "77"}]}),
                        ("mongo", Some("find")) => json!({"docs": [1]}),
                        ("mongo", Some("insert_one")) => json!({"inserted_id": "x"}),
                        ("mongo", Some("delete_one")) => json!({"deleted": 1}),
                        ("redis", Some("set_nx_ex")) => json!({"set": true}),
                        ("redis", Some("lrange")) => json!({"values": ["a"]}),
                        other => panic!("unexpected call {other:?}"),
                    };
                    calls.push(json!({"op": message["op"], "name": message["name"], "input": message["input"]}));
                    let reply = json!({"type": "reply", "call": message["call"], "value": value});
                    stdin.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                }
                Some("done") => break message,
                _ => {}
            }
        }
    })
    .await
    .expect("the extension did not finish");
    (calls, done)
}

#[tokio::test]
async fn the_declared_calls_are_what_the_workers_send_and_return() {
    let values = json!({"owner": "1", "title": "next", "done": false});
    let again = json!({"owner": "1", "title": "again", "done": false});
    // The ids of `apply` are strings on the wire; the optional `key` is dropped by Node and sent as null by Python.
    let data_calls = |todo_key: Value, again_key: Value| {
        json!([
            {"op": "aggregate", "name": "Order.revenue", "input": {"month": "2026-11"}},
            {"op": "read", "name": "Todo", "input": {
                "read": "Todo", "select": ["id", "title", {"owner": {"select": ["id"]}}],
                "filter": [{"field": "id", "op": "eq", "value": "10"}], "sort": [{"field": "id", "dir": "asc"}], "limit": 5}},
            {"op": "apply", "name": "Todo.complete", "input": {"ids": ["10", "11", "12"], "key": "complete-1", "values": {"note": "x"}, "expect": {"note": null}}},
            {"op": "apply", "name": "Todo.complete", "input": {"ids": ["10"]}},
            {"op": "create", "name": "Todo", "input": todo_key},
            {"op": "create", "name": "Todo", "input": again_key},
        ])
    };
    let data_output = json!({"sum": {"total": 5}, "rows": 1, "changed": ["10"], "unchanged": ["11"], "made": "77", "noKey": "77"});
    let external_calls = json!([
        {"op": "mongo", "name": "events", "input": {"command": "find", "filter": {"a": 1}, "projection": {"_id": 0}, "limit": 3}},
        {"op": "mongo", "name": "events", "input": {"command": "insert_one", "document": {"a": 1}, "compensate": true}},
        {"op": "mongo", "name": "events", "input": {"command": "delete_one", "filter": {"a": 1}, "compensate": true, "key": "d-1"}},
        {"op": "redis", "name": "lock:1", "input": {"command": "set_nx_ex", "value": "v", "ttl_seconds": 30, "compensate": true}},
        {"op": "redis", "name": "list:1", "input": {"command": "lrange", "start": 0, "stop": -1}},
    ]);
    let external_output = json!({"found": [1], "inserted": "x", "kept": 1, "flag": true, "items": ["a"]});
    for lang in ["node", "python"] {
        let (calls, done) = drive(lang, "typed.data", json!({"owner": "1"})).await;
        let (todo, no_key) = if lang == "node" {
            (json!({"values": values, "key": "todo-1"}), json!({"values": again}))
        } else {
            (json!({"values": values, "key": "todo-1"}), json!({"values": again, "key": null}))
        };
        // The Python fixture passes the mixed id list `[10, "11"]`; Node passes `[10, "11", 12n]`.
        let mut expected = data_calls(todo, no_key);
        if lang == "python" {
            expected[2]["input"]["ids"] = json!(["10", "11"]);
        }
        assert_eq!(Value::Array(calls), expected, "{lang} data calls");
        assert_eq!(done["output"], data_output, "{lang} data output: {done}");

        let (calls, done) = drive(lang, "typed.external", json!({"owner": "1"})).await;
        assert_eq!(Value::Array(calls), external_calls, "{lang} external calls");
        assert_eq!(done["output"], external_output, "{lang} external output: {done}");

        let (calls, done) = drive(lang, "typed.refuses", json!({"owner": "1"})).await;
        assert!(calls.is_empty());
        assert_eq!(
            (&done["error"]["code"], &done["error"]["appCode"], &done["error"]["message"]),
            (&json!("WIRE.EXTENSION_REJECTED"), &json!("INSUFFICIENT_STOCK"), &json!("재고가 부족합니다")),
            "{lang} reject: {done}"
        );
    }
    let (_, done) = drive("node", "typed.double", json!({"n": 4})).await;
    assert_eq!(done["output"], json!({"double": 8}));
}
