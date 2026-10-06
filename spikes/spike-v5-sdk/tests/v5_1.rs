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

#[test]
fn v5_1_contract_types_infer_and_reject() {
    let facts = load_str(A, Form::A).unwrap().execution;
    let ts = contract_ts(&facts);
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/contract.ts"), &ts).unwrap();
    assert!(!ts.contains("internalNote: string;"), "정책 필드는 null 가능해야 함");
    let (ok, out) = tsc("type-tests.ts");
    assert!(ok, "type-tests.ts 실패:\n{out}");
    let (ok, out) = tsc("type-neg.ts");
    assert!(!ok && out.contains("type-neg.ts"), "음성 대조가 통과해버림:\n{out}");
    eprintln!("tsc type-tests ok; type-neg 실패 확인:\n{}", out.lines().next().unwrap_or(""));
}

/// Codex r4b F03·F04: select에 없는 filter 전용 필드와 Id filter 입력.
#[test]
fn v5_1_filter_only_and_id_input() {
    let src = A
        .replacen("select id, title, periodEnd, views, bookmarkCount, internalNote", "select id, title, views, bookmarkCount, internalNote", 1)
        .replacen("filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, periodEnd.lte, id.eq", 1);
    let facts = load_str(&src, Form::A).unwrap().execution;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/variant");
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/contract.ts"), contract_ts(&facts)).unwrap();
    let adapter = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/aip.ts")).unwrap();
    std::fs::write(format!("{dir}/aip.ts"), adapter.replace("./generic.ts", "../generic.ts")).unwrap();
    std::fs::write(
        format!("{dir}/filter.ts"),
        r#"import { client, type Transport } from "./aip.ts";
declare const send: Transport;
const aip = client(send);
export async function f() {
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "periodEnd", op: "gte", value: "2026-10-09T00:00:00Z" }] });
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "id", op: "eq", value: "100" }] });
  await aip.read({ read: "Recruitment", select: ["id"], filter: [{ field: "id", op: "eq", value: 100 }] });
  // @ts-expect-error periodEnd는 select에 없으니 결과 타입에 없다
  const r = await aip.read({ read: "Recruitment", select: ["periodEnd"] });
}
"#,
    )
    .unwrap();
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
            "filter.ts",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

// in/isNull 연산의 값 타입(배열·bool)이 생성 계약 타입에서 강제되는지 본다.
#[test]
fn filter_operator_value_types() {
    let (ok, out) = tsc("filter-op-types.ts");
    assert!(ok, "{out}");
}
