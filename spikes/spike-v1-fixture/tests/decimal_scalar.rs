use spike_v1_fixture::{digest, load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");
const ETS: &str = include_str!("../fixture/recruitment.e.ts");
const EPY: &str = include_str!("../fixture/recruitment.e.py");
const HTS: &str = include_str!("../fixture/recruitment.h.ts");
const HPY: &str = include_str!("../fixture/recruitment.h.py");

fn replace_once(source: &str, old: &str, new: &str, form: Form) -> String {
    assert_eq!(source.matches(old).count(), 1, "{form:?}: expected one `{old}`");
    source.replacen(old, new, 1)
}

fn decimal_source(form: Form, ty: &str) -> String {
    let (source, old) = match form {
        Form::A => (A, "internalNote: Text?"),
        Form::ETs => (ETS, "internalNote: Text?"),
        Form::EPy => (EPY, "internalNote: Text?"),
        Form::HTs => (HTS, "internalNote: \"Text?\""),
        Form::HPy => (HPY, "\"internalNote\": \"Text?\""),
    };
    replace_once(source, old, &old.replace("Text?", ty), form)
}

#[test]
fn decimal_precision_and_scale_canonicalize_in_every_definition_form() {
    let forms = [Form::A, Form::ETs, Form::EPy, Form::HTs, Form::HPy];
    let mut digests = Vec::new();
    for form in forms {
        let output = load_str(&decimal_source(form, "Decimal(5,2)?"), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}"));
        let fields = &output.execution["resources"]["Recruitment"]["fields"];
        assert_eq!(fields["internalNote"]["ty"], "Decimal<5,2>?");
        assert_eq!(fields["internalNote"].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), ["range", "ty"]);
        digests.push(digest(&output.execution));
    }
    assert!(digests.iter().all(|digest| digest == &digests[0]));
}

#[test]
fn decimal_precision_and_scale_are_bounded_and_consistent() {
    for valid in ["Decimal(1,0)", "Decimal(2,2)", "Decimal(38,18)", "Decimal(38,38)"] {
        load_str(&decimal_source(Form::A, valid), Form::A).unwrap_or_else(|errors| panic!("{valid}: {errors:?}"));
    }
    for invalid in ["Decimal(0,0)", "Decimal(39,2)", "Decimal(2,3)", "Decimal(1,-1)", "Decimal(2,2,1)"] {
        assert!(load_str(&decimal_source(Form::A, invalid), Form::A).is_err(), "{invalid} must be rejected");
    }
}

#[test]
fn decimal_contextual_literals_are_typed_and_enforce_their_declared_digits() {
    let source = replace_once(A, "views: Int", "views: Decimal(5,2)", Form::A);
    let source = replace_once(&source, "to status = CLOSED", "to status = CLOSED, views = \"123.45\"", Form::A);
    let output = load_str(&source, Form::A).unwrap();
    assert_eq!(
        output.execution["resources"]["Recruitment"]["transitions"]["close"]["to"]["views"],
        serde_json::json!({"lit":"123.45","ty":"Decimal<5,2>"})
    );

    for invalid in ["1000.00", "1.234", "NaN", "1e3", "+1", "01.00", "1.", ".5", "١.0"] {
        let bad = source.replace("123.45", invalid);
        assert!(matches!(load_str(&bad, Form::A), Err(errors) if errors.iter().any(|diag| diag.code == "BAD_VALUE")), "{invalid}");
    }
}

#[test]
fn decimal_filters_and_sorting_are_allowed_by_numeric_type() {
    let source = replace_once(A, "views: Int", "views: Decimal(5,2)", Form::A);
    let source = replace_once(&source, "filter periodEnd.gte, periodEnd.lte", "filter views.gte, views.lte", Form::A);
    let output = load_str(&source, Form::A).unwrap();
    let expose = &output.execution["resources"]["Recruitment"]["exposeRead"];
    assert_eq!(output.execution["resources"]["Recruitment"]["fields"]["views"]["ty"], "Decimal<5,2>");
    assert_eq!(expose["filter"], serde_json::json!(["views.gte", "views.lte"]));
    assert!(expose["sort"].as_array().unwrap().iter().any(|field| field == "views"));
}
