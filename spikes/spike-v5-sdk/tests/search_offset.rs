//! contains(string 값)와 opt-in offset이 생성 계약·Query 타입에 반영되는지 본다.
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
fn contract_lists_contains_as_string_and_max_offset_only_when_declared() {
    let src = A.replacen("filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, periodEnd.lte, title.contains", 1).replacen(
        "budget { rows 50; depth 2; deadline 2s; cost 1000 }",
        "budget { rows 50; depth 2; deadline 2s; cost 1000; offset 1000 }",
        1,
    );
    let with = recruitment_block(&contract_ts(&load_str(&src, Form::A).unwrap().execution));
    assert!(with.contains("\"title.contains\""), "{with}");
    assert!(with.contains("title: string;"), "{with}");
    assert!(with.contains("maxOffset: 1000;"), "{with}");
    let base = recruitment_block(&contract_ts(&load_str(A, Form::A).unwrap().execution));
    assert!(!base.contains("maxOffset"), "선언이 없으면 계약에도 없다: {base}");
}

#[test]
fn query_and_filter_types_enforce_contains_value_and_offset_opt_in() {
    let (ok, out) = tsc("search-offset-types.ts");
    assert!(ok, "{out}");
}
