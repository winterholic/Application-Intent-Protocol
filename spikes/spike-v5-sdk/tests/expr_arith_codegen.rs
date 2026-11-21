//! P0-B 제한 산술의 SDK 생성물: TS 계약의 `input` 타입과 range 주석, 계약 지문(input 전이가 있는 정의만 변함),
//! Python helper의 `input=`(test-plan B-S5, V-1, V-2).
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_module_python, contract_module_with_apply, contract_ts_with_apply};
use std::io::Write;
use std::process::{Command, Stdio};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000); cash: Decimal(12,2); likes: Int }
  rows read when owner = actor
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow owner = actor
    to stock = stock - input.qty
  }
  transition pay {
    input { amount: Decimal(12,2); note: Int(1..9) }
    from cash >= input.amount
    allow owner = actor
    to cash = cash - input.amount
  }
  transition like { from true; allow owner = actor; to likes = likes + 1 }
  expose apply take { target id; bulk maxRows 5 }
  expose apply pay { target id; bulk maxRows 5 }
  expose apply like { target id; bulk maxRows 5 }
}
"#;

fn facts(spec: &str) -> serde_json::Value {
    load_str(spec, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn action_block<'a>(ts: &'a str, name: &str) -> &'a str {
    let start = ts.find(&format!("  \"Product.{name}\": {{")).unwrap_or_else(|| panic!("Product.{name} missing in\n{ts}"));
    ts[start..].split("\n  };").next().unwrap()
}

#[test]
fn the_contract_types_declare_input_with_its_range_only_for_transitions_that_declare_it() {
    let ts = contract_ts_with_apply(&facts(SPEC), IdWire::DecimalString);
    let take = action_block(&ts, "take");
    assert!(take.contains("input: {\n      qty: number; /** 1..100 */\n    };"), "{take}");
    let pay = action_block(&ts, "pay");
    assert!(pay.contains("amount: string;") && pay.contains("note: number;"), "{pay}");
    assert!(!action_block(&ts, "like").contains("input"), "input이 없는 전이에는 input 타입이 없다");
    // 호출자가 보내는 값의 검사에 쓰는 설명자도 같은 정보를 담는다.
    assert!(ts.contains("\"Product.take\":{\"input\":{\"qty\":{\"range\":[1,100],\"ty\":\"Int\"}}}"), "{ts}");
}

#[test]
fn b_s5_definitions_without_input_do_not_mention_it_and_the_fingerprint_moves_only_with_input() {
    let legacy = SPEC.replace("input { qty: Int(1..100) }\n    from stock >= input.qty", "from true").replace("stock - input.qty", "stock - 1");
    let legacy = legacy
        .replace("input { amount: Decimal(12,2); note: Int(1..9) }\n    from cash >= input.amount", "from true")
        .replace("cash - input.amount", "cash - 1");
    let facts_legacy = facts(&legacy);
    for wire in [IdWire::SafeNumber, IdWire::DecimalString, IdWire::Legacy] {
        let ts = contract_ts_with_apply(&facts_legacy, wire);
        assert!(!ts.contains("input: {") && !ts.contains("\"input\""), "input을 쓰지 않는 정의의 계약에는 input이 없다: {wire:?}");
        let fp = contract_fingerprint_with_apply(&facts_legacy, wire);
        // input 전이를 쓰는 정의는 계약이 다르다.
        assert_ne!(fp, contract_fingerprint_with_apply(&facts(SPEC), wire));
        // 같은 정의는 같은 지문이다.
        assert_eq!(fp, contract_fingerprint_with_apply(&facts(&legacy), wire));
    }
    // 입력 선언(타입·범위·이름)과 항 사용이 지문에 반영되고, 정책(allow)만 바뀐 것은 반영되지 않는다.
    let fp = contract_fingerprint_with_apply(&facts(SPEC), IdWire::DecimalString);
    for changed in [
        SPEC.replace("Int(1..100)", "Int(1..101)"),
        SPEC.replace("qty: Int(1..100)", "units: Int(1..100)").replace("input.qty", "input.units"),
        SPEC.replace("note: Int(1..9)", "note: Int(1..9); extra: Int(1..9)"),
    ] {
        assert_ne!(contract_fingerprint_with_apply(&facts(&changed), IdWire::DecimalString), fp);
    }
    let policy_only = SPEC.replace("allow owner = actor\n    to stock = stock - input.qty", "allow actor != null\n    to stock = stock - input.qty");
    assert_eq!(contract_fingerprint_with_apply(&facts(&policy_only), IdWire::DecimalString), fp);
}

fn tsc_path() -> String {
    let local = concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-0-ts/node_modules/.bin/tsc");
    if std::path::Path::new(local).exists() {
        return local.to_string();
    }
    // 새 worktree에는 node_modules가 없다. 설치된 tsc 경로를 AIP_TSC로 준다.
    std::env::var("AIP_TSC").unwrap_or_else(|_| panic!("tsc가 없음: spikes/spike-0-ts에서 npm ci를 하거나 AIP_TSC를 지정한다"))
}

fn tsc(dir: &std::path::Path, file: &str) -> (bool, String) {
    let out = Command::new(tsc_path())
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
fn typescript_apply_requests_require_the_declared_input_and_reject_everything_else() {
    let generic = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/generic.ts");
    let dir = std::env::temp_dir().join(format!("aip-wp1-expr-arith-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("contract.ts"), contract_module_with_apply(&facts(SPEC), generic, IdWire::DecimalString)).unwrap();
    let header = format!(
        "import type {{ ApplyRequest }} from {generic:?};\nimport type {{ ApplyContract }} from \"./contract.ts\";\ntype R = ApplyRequest<ApplyContract>;\n"
    );
    std::fs::write(
        dir.join("ok.ts"),
        format!(
            "{header}
const take: R = {{ apply: \"Product.take\", target: {{ ids: [\"1\" as any] }}, input: {{ qty: 3 }} }};
const pay: R = {{ apply: \"Product.pay\", target: {{ ids: [\"1\" as any] }}, input: {{ amount: \"12.50\", note: 1 }} }};
const like: R = {{ apply: \"Product.like\", target: {{ ids: [\"1\" as any] }} }};
export {{ take, pay, like }};
"
        ),
    )
    .unwrap();
    let (ok, out) = tsc(&dir, "ok.ts");
    assert!(ok, "정상 요청이 컴파일되지 않음:\n{out}");
    for (name, body) in [
        ("missing-input", "{ apply: \"Product.take\", target: { ids: [\"1\" as any] } }"),
        ("wrong-type", "{ apply: \"Product.take\", target: { ids: [\"1\" as any] }, input: { qty: \"3\" } }"),
        ("unknown-key", "{ apply: \"Product.take\", target: { ids: [\"1\" as any] }, input: { qty: 3, extra: 1 } }"),
        ("decimal-as-number", "{ apply: \"Product.pay\", target: { ids: [\"1\" as any] }, input: { amount: 12.5, note: 1 } }"),
        ("input-on-plain-transition", "{ apply: \"Product.like\", target: { ids: [\"1\" as any] }, input: { qty: 1 } }"),
    ] {
        std::fs::write(dir.join(format!("{name}.ts")), format!("{header}const bad: R = {body};\nexport {{ bad }};\n")).unwrap();
        let (ok, out) = tsc(&dir, &format!("{name}.ts"));
        assert!(!ok, "{name}: 잘못된 요청이 컴파일됨");
        assert!(out.contains("error TS"), "{name}: {out}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn python_helper_takes_the_declared_input_as_a_keyword_and_sends_it() {
    let generated = contract_module_python(&facts(SPEC), IdWire::DecimalString);
    for expected in [
        "ProductTakeInput = TypedDict(\"ProductTakeInput\", {\n    \"qty\": int,\n})",
        "ProductPayInput = TypedDict(\"ProductPayInput\", {\n    \"amount\": str,\n    \"note\": int,\n})",
        "\"input\": ProductTakeInput",
        "def apply_product_take(client: Client, target: ProductTakeIds, *, input: ProductTakeInput, key: str | None = None) -> dict:",
        "def apply_product_like(client: Client, target: ProductLikeIds, *, key: str | None = None) -> dict:",
    ] {
        assert!(generated.contains(expected), "missing `{expected}`\n{generated}");
    }
    let script = r#"
import sys
sys.path.insert(0, sys.argv[1])
scope = {}
exec(sys.stdin.read(), scope)
class Stub:
    def __init__(self): self.calls = []
    def apply(self, request, key=None): self.calls.append((request, key)); return {"ok": True}
stub = Stub()
scope["apply_product_take"](stub, {"ids": ["1"]}, input={"qty": 3}, key="k1")
scope["apply_product_pay"](stub, {"ids": ["1"]}, input={"amount": "12.50", "note": 1})
scope["apply_product_like"](stub, {"ids": ["1"]})
assert stub.calls[0] == ({"apply": "Product.take", "target": {"ids": ["1"]}, "input": {"qty": 3}}, "k1"), stub.calls
assert stub.calls[1][0]["input"] == {"amount": "12.50", "note": 1}, stub.calls
assert "input" not in stub.calls[2][0], stub.calls
try:
    scope["apply_product_take"](stub, {"ids": ["1"]}, key="k2")
except TypeError:
    pass
else:
    raise SystemExit("input= 없이 부를 수 있음")
"#;
    let sdk = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let mut child = Command::new("python3").args(["-c", script, sdk]).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("python3");
    child.stdin.take().unwrap().write_all(generated.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // input이 없는 정의의 Python 계약에는 Input 타입이 없다.
    let plain = contract_module_python(
        &facts(
            &SPEC
                .replace("input { qty: Int(1..100) }\n    from stock >= input.qty", "from true")
                .replace("stock - input.qty", "stock - 1")
                .replace("input { amount: Decimal(12,2); note: Int(1..9) }\n    from cash >= input.amount", "from true")
                .replace("cash - input.amount", "cash - 1"),
        ),
        IdWire::DecimalString,
    );
    assert!(!plain.contains("input"), "{plain}");
}
