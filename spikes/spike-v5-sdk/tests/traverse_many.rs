//! 1:N traverse가 생성 계약(traverseMany)·Query/Row 타입(배열)·읽기 descriptor에 반영되는지 본다.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_module_with_extensions, contract_ts};
use std::process::Command;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

const DEF: &str = "
enum CommentState { VISIBLE, HIDDEN }
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Id; title: Text(1..100) }
  rows read when true
  expose read {
    select id, title
    traverse comments via Comment.post { select id, body, secret; sort id; limit 20 }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
resource Comment {
  fields { id: Id; post: Post; author: Member; body: Text(1..200); secret: Text?; state: CommentState }
  rows read when state = VISIBLE
  field secret read when author = actor
  expose read { select id, body, secret, state }
}
";

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
fn contract_lists_traverse_many_only_when_declared() {
    let ts = contract_ts(&load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution);
    let post = ts.split("  Post: {").nth(1).unwrap().split("\n  };").next().unwrap();
    assert!(post.contains("traverseMany: { comments: { target: \"Comment\"; select: \"body\" | \"id\" | \"secret\"; maxLimit: 20 } };"), "{post}");
    // 정렬·내부 재적용 표식은 공개 계약에 나오지 않는다
    assert!(!post.contains("reapply") && !post.contains("rowRead"), "{post}");
    let base = contract_ts(&load_str(A, Form::A).unwrap().execution);
    assert!(!base.contains("traverseMany"), "선언이 없으면 계약에도 없다");
}

#[test]
fn types_make_the_child_field_an_array() {
    let (ok, out) = tsc("traverse-many-types.ts");
    assert!(ok, "{out}");
}

#[test]
fn read_descriptor_exposes_target_select_and_limit_only() {
    let f = load_str(DEF, Form::A).unwrap().execution;
    let module = contract_module_with_extensions(&f, "./generic.ts", IdWire::Legacy);
    assert!(module.contains("traverseMany"), "{module}");
    assert!(!module.contains("reapply"), "내부 표식이 공개 descriptor에 새면 안 됨");
}
