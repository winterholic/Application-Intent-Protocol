//! budget `cursor` opt-in이 생성 계약과 Query 타입(after)에 반영되는지 본다.
use spike_v1_fixture::{load_str, Form};
use spike_v5_sdk::contract_ts;
use std::process::Command;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn tsc(file: &str) -> (bool, String) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk");
    let tsc = concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-0-ts/node_modules/.bin/tsc");
    let out = Command::new(tsc)
        .current_dir(dir)
        .args([
            "--noEmit",
            "--strict",
            "--target",
            "esnext",
            "--module",
            "nodenext",
            "--moduleResolution",
            "nodenext",
            "--allowImportingTsExtensions",
            "--skipLibCheck",
            file,
        ])
        .output()
        .expect("tsc 실행");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string())
}

fn recruitment_block(ts: &str) -> String {
    let start = ts.find("  Recruitment: {").expect("Recruitment");
    ts[start..].split("\n  };").next().unwrap().to_string()
}

#[test]
fn contract_lists_cursor_only_when_declared() {
    let src = A.replacen("budget { rows 50; depth 2; deadline 2s; cost 1000 }", "budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }", 1);
    let with = recruitment_block(&contract_ts(&load_str(&src, Form::A).unwrap().execution));
    assert!(with.contains("cursor: true;"), "{with}");
    let base = recruitment_block(&contract_ts(&load_str(A, Form::A).unwrap().execution));
    assert!(!base.contains("cursor"), "선언이 없으면 계약에도 없다: {base}");
}

#[test]
fn query_type_opens_after_only_for_cursor_resources() {
    let (ok, out) = tsc("cursor-types.ts");
    assert!(ok, "{out}");
}

#[test]
fn client_read_accepts_after_for_declared_contract() {
    // 생성 계약을 쓰는 client()의 read도 같은 조건부 타입을 쓴다.
    let aip = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/aip.ts")).unwrap();
    let variant = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/variant/aip.ts")).unwrap();
    for (n, src) in [("aip.ts", aip), ("variant/aip.ts", variant)] {
        assert!(src.contains("G.CursorOpt<Contract, R>"), "{n}에 CursorOpt가 없음");
    }
}
