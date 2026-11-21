//! P0-B 제한 산술: `input` 블록과 `f = f ± 항` 한 형태(test-plan B-S1~S5, B-F1~F8).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
resource Wallet {
  fields { id: Id; owner: Member; funds: Decimal(12,2); credit: Decimal(12,4); count: Int }
  rows read when true
}
resource Product {
  fields {
    id: Id; owner: Member; stock: Int(0..1000); reserved: Int; maybe: Int?; likes: Int; risk: Int
    balance: Decimal(12,2); fine: Decimal(12,4); total: Decimal(12,2); label: Text(1..20)
  }
  rows read when true
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow actor != null
    to stock = stock - input.qty
  }
  transition pay {
    input { amount: Decimal(12,2) }
    from balance >= input.amount
    allow actor != null
    to balance = balance - input.amount
  }
  transition topup {
    input { points: Int(1..100) }
    from true
    allow actor != null
    to balance = balance + input.points
  }
  transition reserve {
    from true
    allow actor != null
    to stock = stock - this.reserved
  }
  transition settle {
    from true
    allow actor != null
    to balance = balance - this.total
    update Wallet where owner = this.owner { funds = funds - this.total }
  }
  transition literal {
    from true
    allow actor != null
    to likes = likes + 1, balance = balance + \"1.50\"
  }
  expose apply take { target id; bulk maxRows 5 }
  expose apply pay { target id; bulk maxRows 5 }
  expose apply topup { target id; bulk maxRows 5 }
  expose apply reserve { target id; bulk maxRows 5 }
  expose apply settle { target id; bulk maxRows 5 }
  expose apply literal { target id; bulk maxRows 5 }
}
";

fn facts(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn code_of(src: &str) -> String {
    match load_str(src, Form::A) {
        Ok(_) => panic!("accepted:\n{src}"),
        Err(ds) => ds.iter().map(|d| format!("{} {}", d.code, d.msg)).collect::<Vec<_>>().join(" | "),
    }
}

fn expect_code(src: &str, code: &str) {
    let got = code_of(src);
    assert!(got.contains(code), "want {code}, got {got}\n{src}");
}

fn tr<'a>(f: &'a Value, res: &str, name: &str) -> &'a Value {
    &f["resources"][res]["transitions"][name]
}

#[test]
fn b_s1_s2_s4_input_terms_become_arith_facts_with_declared_input() {
    let f = facts(DEF);
    assert_eq!(tr(&f, "Product", "take")["to"]["stock"], json!({ "arith": { "op": "-", "term": { "input": "qty" } } }));
    assert_eq!(tr(&f, "Product", "take")["input"], json!([["qty", "Int", [1, 100]]]));
    assert_eq!(tr(&f, "Product", "pay")["to"]["balance"], json!({ "arith": { "op": "-", "term": { "input": "amount" } } }));
    assert_eq!(tr(&f, "Product", "pay")["input"], json!([["amount", "Decimal<12,2>", null]]));
    // B-S4: Int 항을 Decimal 필드에.
    assert_eq!(tr(&f, "Product", "topup")["to"]["balance"], json!({ "arith": { "op": "+", "term": { "input": "points" } } }));
    // from은 input과 비교하고 runtime이 잠근 뒤 판정하도록 facts에 input 경로로 남는다.
    assert_eq!(tr(&f, "Product", "take")["from"]["r"]["path"]["root"], "input.qty");
}

#[test]
fn same_row_field_terms_and_literals() {
    let f = facts(DEF);
    let this_term = &tr(&f, "Product", "reserve")["to"]["stock"]["arith"];
    assert_eq!(this_term["op"], "-");
    assert_eq!(this_term["term"]["path"], json!({ "root": "this", "segs": ["reserved"] }));
    // Int 리터럴은 Int 필드에서만 기존 increment 모양이고, Decimal 필드와 Decimal 리터럴은 arith다.
    assert_eq!(tr(&f, "Product", "literal")["to"]["likes"], json!({ "increment": 1 }));
    assert_eq!(tr(&f, "Product", "literal")["to"]["balance"], json!({ "arith": { "op": "+", "term": { "lit": "1.50" } } }));
}

#[test]
fn b_s3_update_effect_arith_targets_the_effect_row_and_terms_read_the_transition_row() {
    let f = facts(DEF);
    let effect = &tr(&f, "Product", "settle")["effects"][0];
    assert_eq!(effect["update"], "Wallet");
    assert_eq!(effect["values"]["funds"]["arith"]["op"], "-");
    assert_eq!(effect["values"]["funds"]["arith"]["term"]["path"], json!({ "root": "this", "segs": ["total"] }));
}

#[test]
fn b_s5_existing_definitions_keep_their_facts() {
    // 증감 리터럴은 increment, input을 선언하지 않은 전이에는 input 키가 없다.
    let legacy = "resource Member { fields { id: Id } } actor Member
resource Item { fields { id: Id; stock: Int(0..9) } rows read when true
  transition sell { from stock > 0; allow actor != null; to stock = stock - 1 }
  expose apply sell { target id; bulk maxRows 1 } }";
    let f = facts(legacy);
    assert_eq!(tr(&f, "Item", "sell")["to"]["stock"], json!({ "increment": -1 }));
    assert!(tr(&f, "Item", "sell").get("input").is_none(), "{}", tr(&f, "Item", "sell"));
    assert!(tr(&f, "Item", "sell")["to"]["stock"].get("arith").is_none());
}

#[test]
fn five_forms_give_identical_facts_and_digest() {
    let text = "resource Member { fields { id: Id } } actor Member
resource Item { fields { id: Id; stock: Int(0..9); cash: Decimal(12,2) } rows read when true
  transition take { input { qty: Int(1..5) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  transition pay { from true; allow actor != null; update Wallet where owner = this.owner { cash = cash - this.cash } }
  expose apply take { target id; bulk maxRows 1 } }
resource Wallet { fields { id: Id; owner: Member; cash: Decimal(12,2) } rows read when true }";
    let text = text.replace("update Wallet where owner = this.owner { cash = cash - this.cash }", "to stock = stock + 1");
    let expected = load_str(&text, Form::A).expect("native");
    let obj = json!({"actor":"Member","resources":{
      "Member":{"fields":{"id":"Id"}},
      "Item":{"fields":{"id":"Id","stock":"Int(0..9)","cash":"Decimal(12,2)"},"rows":"true",
        "transitions":{
          "take":{"input":{"qty":"Int(1..5)"},"from":"stock >= input.qty","allow":"actor != null","to":"stock = stock - input.qty"},
          "pay":{"from":"true","allow":"actor != null","to":"stock = stock + 1"}},
        "exposeApply":{"take":{"target":["id"],"bulkMaxRows":1}}},
      "Wallet":{"fields":{"id":"Id","owner":"Member","cash":"Decimal(12,2)"},"rows":"true"}
    }});
    for (form, source) in [
        (Form::ETs, format!("import {{ aip }} from \"@aip/define\"; export default aip`{text}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip('''{text}''')")),
        (Form::HTs, format!("import {{ define }} from \"@aip/define\"; export const spec = define({obj});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({obj})")),
    ] {
        let actual = load_str(&source, form).unwrap_or_else(|e| panic!("{form:?}: {e:?}"));
        assert_eq!(actual.execution, expected.execution, "{form:?}");
        assert_eq!(spike_v1_fixture::digest(&actual.execution), spike_v1_fixture::digest(&expected.execution));
    }
    assert_eq!(tr(&expected.execution, "Item", "take")["input"], json!([["qty", "Int", [1, 5]]]));
}

#[test]
fn host_form_effect_update_accepts_arith_like_the_native_form() {
    let native = "resource Member { fields { id: Id } } actor Member
resource Wallet { fields { id: Id; owner: Member; cash: Decimal(12,2) } rows read when true }
resource Order { fields { id: Id; owner: Member; total: Decimal(12,2) } rows read when true
  transition pay { from true; allow actor != null; update Wallet where owner = this.owner { cash = cash - this.total }; to total = total + \"0.01\" }
  expose apply pay { target id; bulk maxRows 1 } }";
    let expected = facts(native);
    let obj = json!({"actor":"Member","resources":{
      "Member":{"fields":{"id":"Id"}},
      "Wallet":{"fields":{"id":"Id","owner":"Member","cash":"Decimal(12,2)"},"rows":"true"},
      "Order":{"fields":{"id":"Id","owner":"Member","total":"Decimal(12,2)"},"rows":"true",
        "transitions":{"pay":{"from":"true","allow":"actor != null","to":"total = total + \"0.01\"",
          "effects":[{"update":"Wallet","where":{"owner":"this.owner"},"values":{"cash":"cash - this.total"}}]}},
        "exposeApply":{"pay":{"target":["id"],"bulkMaxRows":1}}}
    }});
    let actual = load_str(&format!("import {{ define }} from \"@aip/define\"; export const spec = define({obj});"), Form::HTs).expect("host form");
    assert_eq!(actual.execution, expected);
}

// ---- 실패 fixture ----

#[test]
fn b_f1_multiplication_is_not_a_token() {
    expect_code(&DEF.replace("to stock = stock - input.qty", "to stock = stock * 2"), "DEF.LEX_UNEXPECTED_CHAR");
    expect_code(&DEF.replace("funds = funds - this.total }", "funds = funds * this.total }"), "DEF.LEX_UNEXPECTED_CHAR");
}

#[test]
fn b_f2_unknown_input_name() {
    expect_code(&DEF.replace("to stock = stock - input.qty", "to stock = stock - input.missing"), "DEF.UNKNOWN_FIELD");
    // input 선언이 없는 전이에서 input.p를 쓰면 같은 오류다.
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock - input.qty"), "DEF.UNKNOWN_FIELD");
}

#[test]
fn b_f3_one_term_of_the_same_field_only() {
    for (old, new) in [
        ("to stock = stock - input.qty", "to stock = stock - input.qty - input.qty"),
        ("to stock = stock - input.qty", "to stock = stock - this.reserved - this.likes"),
        ("to stock = stock - input.qty", "to stock = reserved - 1"),
        ("to stock = stock - input.qty", "to stock = reserved - input.qty"),
        ("to stock = stock - input.qty", "to stock = input.qty - stock"),
        ("to stock = stock - input.qty", "to stock = stock - 0"),
        ("to stock = stock - input.qty", "to stock = stock + stock"),
        ("to stock = stock - input.qty", "to stock = stock + actor"),
        ("funds = funds - this.total }", "funds = funds - this.total - this.total }"),
        ("funds = funds - this.total }", "funds = this.funds - this.total }"),
        ("funds = funds - this.total }", "funds = total - this.total }"),
    ] {
        expect_code(&DEF.replace(old, new), "DEF.ARITH_NOT_ALLOWED");
    }
    // 조건식에서는 여전히 계산하지 않는다.
    expect_code(&DEF.replace("from stock >= input.qty", "from stock + 1 >= input.qty"), "DEF.ARITH_NOT_ALLOWED");
}

#[test]
fn b_f4_null_is_excluded_at_definition_time() {
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Int(1..100)? }"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Text }"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Bool }"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock - this.maybe"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("stock: Int(0..1000);", "stock: Int(0..1000)?;"), "DEF.TYPE_MISMATCH");
    expect_code(
        &DEF.replace("label: Text(1..20)", "label: Text(1..20)\n    reserved2: Int?")
            .replace("to stock = stock - this.reserved", "to stock = stock - this.reserved2"),
        "DEF.TYPE_MISMATCH",
    );
}

#[test]
fn b_f5_decimal_scale_and_int_field_rules() {
    // Decimal(12,4) 항을 Decimal(12,2) 필드에.
    expect_code(&DEF.replace("to balance = balance + input.points", "to balance = balance + this.fine"), "DEF.TYPE_MISMATCH");
    // 자릿수가 같거나 작은 Decimal 항, Int 항은 된다.
    facts(&DEF.replace("input { points: Int(1..100) }", "input { points: Decimal(12,1) }"));
    expect_code(&DEF.replace("input { points: Int(1..100) }", "input { points: Decimal(12,3) }"), "DEF.TYPE_MISMATCH");
    // Ref 항은 숫자가 아니다.
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock + this.owner"), "DEF.TYPE_MISMATCH");
    // Decimal 항을 Int 필드에.
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock - this.total"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Decimal(12,2) }"), "DEF.TYPE_MISMATCH");
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock - \"1.00\""), "DEF.TYPE_MISMATCH");
    // 정수 필드가 아닌 대상.
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to label = label - 1"), "DEF.TYPE_MISMATCH");
    // Decimal 리터럴 형식과 자릿수.
    expect_code(&DEF.replace("balance = balance + \"1.50\"", "balance = balance + \"1.505\""), "DEF.BAD_VALUE");
}

#[test]
fn b_f6_fields_with_read_policy_are_not_arithmetic_operands() {
    let hidden = |field: &str| {
        DEF.replace(
            "rows read when true\n  transition take",
            &format!("rows read when true\n  field {field} read when owner = actor\n  transition take"),
        )
    };
    // 가려진 대상(input 항)과 가려진 항(this.g) 둘 다.
    expect_code(&hidden("stock"), "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
    expect_code(&hidden("reserved"), "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
    expect_code(&hidden("total"), "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
    // 효과 update의 대상 필드에도 같다.
    let wallet = DEF.replace(
        "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true",
        "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true\n  field funds read when owner = actor",
    );
    expect_code(&wallet, "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
}

#[test]
fn b_f7_repeat_unchanged_with_arith_is_rejected_like_increments() {
    expect_code(&DEF.replace("to stock = stock - input.qty", "to stock = stock - input.qty\n    repeat unchanged"), "DEF.UNSUPPORTED");
    expect_code(&DEF.replace("to stock = stock - this.reserved", "to stock = stock - this.reserved\n    repeat unchanged"), "DEF.UNSUPPORTED");
}

#[test]
fn b_f8_compose_cannot_call_a_transition_with_input() {
    let composed = DEF.replace("expose apply take", "expose compose { bulk maxRows 2; transitions take } expose apply take");
    expect_code(&composed, "DEF.UNSUPPORTED");
    // input이 없는 전이는 계속 compose할 수 있다.
    facts(&DEF.replace("expose apply take", "expose compose { bulk maxRows 2; transitions reserve } expose apply take"));
}

#[test]
fn input_declaration_limits_and_shapes() {
    let nine: String = (0..9).map(|n| format!("q{n}: Int(1..9); ")).collect();
    expect_code(&DEF.replace("input { qty: Int(1..100) }", &format!("input {{ qty: Int(1..100); {nine} }}")), "DEF.UNSUPPORTED");
    let eight: String = (0..7).map(|n| format!("q{n}: Int(1..9); ")).collect();
    facts(&DEF.replace("input { qty: Int(1..100) }", &format!("input {{ qty: Int(1..100); {eight} }}")));
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Int(1..100); qty: Int(1..9) }"), "DEF.DUPLICATE");
    // input을 그대로 대입하는 것은 patch와 같은 의도라 받지 않는다.
    expect_code(&DEF.replace("to stock = stock - input.qty", "to stock = input.qty"), "DEF.UNSUPPORTED");
    // 같은 키를 두 번 선언하지 못한다.
    expect_code(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Int(1..100) }\n    input { qty: Int(1..9) }"), "DEF.DUPLICATE");
    // 효과 값(create)에는 input을 쓸 수 없다.
    expect_code(
        &DEF.replace(
            "update Wallet where owner = this.owner { funds = funds - this.total }",
            "update Wallet where owner = this.owner { count = input.qty }",
        ),
        "DEF.UNKNOWN_FIELD",
    );
}

#[test]
fn input_may_feed_effect_arith_terms() {
    let src = DEF.replace(
        "to stock = stock - input.qty",
        "to stock = stock - input.qty\n    update Wallet where owner = this.owner { count = count + input.qty }",
    );
    let f = facts(&src);
    assert_eq!(tr(&f, "Product", "take")["effects"][0]["values"]["count"], json!({ "arith": { "op": "+", "term": { "input": "qty" } } }));
}

// ---- 독립 검토(review-decisions 3절) 반영: B-1 ~ B-4 ----

/// B-1: 항은 양수만 받는다. 방향은 `+`·`-`가 정하므로 음수·0 리터럴이나 하한 없는 Int input은 정의에서 막는다.
#[test]
fn b1_terms_are_positive_int_input_needs_a_lower_bound_of_one() {
    for range in ["Int", "Int(0..100)", "Int(-5..100)", "Int(-100..-1)"] {
        expect_code(&DEF.replace("input { qty: Int(1..100) }", &format!("input {{ qty: {range} }}")), "DEF.BAD_RANGE");
    }
    facts(&DEF.replace("input { qty: Int(1..100) }", "input { qty: Int(1..1) }"));
    // 음수·0 리터럴 항. 음수 정수 리터럴은 문법에 없어 파서가 막고, 0과 Decimal 문자열은 sema가 막는다.
    expect_code(&DEF.replace("to likes = likes + 1, balance = balance + \"1.50\"", "to balance = balance + 0"), "DEF.ARITH_NOT_ALLOWED");
    for term in ["\"-1.50\"", "\"0.00\"", "\"0\""] {
        expect_code(&DEF.replace("balance = balance + \"1.50\"", &format!("balance = balance + {term}")), "DEF.ARITH_NOT_ALLOWED");
    }
    facts(&DEF.replace("balance = balance + \"1.50\"", "balance = balance + \"0.01\""));
}

/// B-2: `allow`·`from`·효과 `when` 어디서든 `input.*`과 가려진 필드를 같은 비교식에 쓰면 이분 탐색 통로가 된다.
#[test]
fn b2_allow_cannot_compare_input_with_a_hidden_field() {
    let hidden =
        DEF.replace("rows read when true\n  transition take", "rows read when true\n  field risk read when owner = actor\n  transition take");
    // 가려진 필드는 risk다. take의 allow만 reserved와 input을 견주게 바꾼다.
    for allow in [
        "allow actor != null and risk >= input.qty",
        "allow input.qty <= risk",
        "allow risk in (input.qty, 3)",
        "allow actor != null and (this.risk >= 0 or input.qty > risk)",
    ] {
        let src = hidden.replace("allow actor != null\n    to stock = stock - input.qty", &format!("{allow}\n    to stock = stock - input.qty"));
        let got = code_of(&src);
        assert!(got.contains("DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION") || got.contains("DEF.ARITH_NOT_ALLOWED"), "{allow}: {got}");
    }
    // 두 비교식이 따로 and로 묶인 것은 호출자가 가려진 값을 상수와 견주는 기존 한 비트라 막지 않는다.
    let separate = hidden
        .replace("allow actor != null\n    to stock = stock - input.qty", "allow risk >= 1 and input.qty <= 5\n    to stock = stock - input.qty");
    facts(&separate);
    // 가려지지 않은 필드와 input의 비교(allow)는 된다.
    facts(&DEF.replace(
        "allow actor != null\n    to stock = stock - input.qty",
        "allow actor != null and stock >= input.qty\n    to stock = stock - input.qty",
    ));
    // exists 안쪽 resource의 가려진 필드도 같다.
    let exists = DEF
        .replace(
            "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true",
            "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true\n  field count read when owner = actor",
        )
        .replace(
            "allow actor != null\n    to stock = stock - input.qty",
            "allow exists Wallet where count >= input.qty\n    to stock = stock - input.qty",
        );
    expect_code(&exists, "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
}

#[test]
fn b2_from_and_effect_when_keep_refusing_the_same_mix() {
    let hidden =
        DEF.replace("rows read when true\n  transition take", "rows read when true\n  field risk read when owner = actor\n  transition take");
    expect_code(&hidden.replace("from stock >= input.qty", "from risk >= input.qty"), "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION");
    // 효과 when에서는 input이 보이지 않는다. 호출자 값이 효과 조건으로 들어가는 길 자체가 없다.
    let when = DEF.replace(
        "update Wallet where owner = this.owner { funds = funds - this.total }",
        "update Wallet where owner = this.owner when input.qty > 1 { funds = funds - this.total }",
    );
    let got = code_of(&when);
    assert!(got.contains("DEF.UNKNOWN_FIELD") || got.contains("DEF.PARSE") || got.contains("DEF.UNRESOLVED"), "{got}");
}

/// B-3: 효과 update의 앞 `f`는 효과 대상 행이다. `this.f`는 전이 행이므로 앞에 올 수 없고, 전이 행에 같은 이름이 있으면 모호하다.
#[test]
fn b3_effect_leading_operand_is_the_target_rows_bare_field() {
    expect_code(&DEF.replace("funds = funds - this.total }", "funds = this.funds - this.total }"), "DEF.ARITH_NOT_ALLOWED");
    // 전이 행(Product)에도 balance가 있는 Wallet 필드를 효과로 줄이면 모호하다.
    let clash = DEF
        .replace("funds: Decimal(12,2); credit", "balance: Decimal(12,2); credit")
        .replace("funds = funds - this.total", "balance = balance - this.total");
    expect_code(&clash, "DEF.AMBIGUOUS_NAME");
}

/// B-4: 효과로 줄이는 Decimal 필드에 하한을 지키는 `check`가 없으면 개발 조언을 낸다(Int range는 사후 검사가 지킨다).
#[test]
fn b4_effect_decrease_without_a_lower_bound_check_is_advised() {
    let advisories =
        |src: &str| -> Vec<&'static str> { load_str(src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).advisories.iter().map(|d| d.code).collect() };
    assert_eq!(advisories(DEF), ["DEF.ADVISORY_EFFECT_NO_LOWER_BOUND"]);
    let guarded = DEF.replacen(
        "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true",
        "funds: Decimal(12,2); credit: Decimal(12,4); count: Int }\n  rows read when true\n  check fundsNotNegative when funds >= \"0.00\"",
        1,
    );
    assert!(advisories(&guarded).is_empty(), "{:?}", advisories(&guarded));
    // 더하는 효과(항은 양수)는 하한을 깎지 못하므로 조언하지 않는다.
    assert!(advisories(&DEF.replace("funds = funds - this.total", "funds = funds + this.total")).is_empty());
    // Int 필드는 range 사후 검사가 지킨다.
    let int_effect = DEF.replace(
        "update Wallet where owner = this.owner { funds = funds - this.total }",
        "update Wallet where owner = this.owner { count = count - 1 }",
    );
    assert!(advisories(&int_effect.replace("count: Int }", "count: Int(0..1000) }")).is_empty());
}
