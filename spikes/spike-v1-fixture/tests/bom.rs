//! A UTF-8 byte order mark at the start of a definition file is not part of the definition. Windows editors write one,
//! and before this the five forms disagreed about it: `.aip` failed with DEF.LEX_UNEXPECTED_CHAR at column 1, `.e.py` and
//! `.h.py` with a misleading DEF.NON_LITERAL, while `.e.ts` and `.h.ts` accepted it. Every form now ignores one leading BOM
//! and produces the same contract and positions as the file without it.
use spike_v1_fixture::{load_str, Form};

const BOM: &str = "\u{feff}";

const FORMS: [(&str, Form, &str); 5] = [
    ("recruitment.aip", Form::A, include_str!("../fixture/recruitment.aip")),
    ("recruitment.e.ts", Form::ETs, include_str!("../fixture/recruitment.e.ts")),
    ("recruitment.e.py", Form::EPy, include_str!("../fixture/recruitment.e.py")),
    ("recruitment.h.ts", Form::HTs, include_str!("../fixture/recruitment.h.ts")),
    ("recruitment.h.py", Form::HPy, include_str!("../fixture/recruitment.h.py")),
];

#[test]
fn a_leading_bom_does_not_change_the_contract_of_any_form() {
    for (name, form, src) in FORMS {
        let plain = load_str(src, form).unwrap_or_else(|e| panic!("{name}: {e:?}")).execution;
        let with_bom = load_str(&format!("{BOM}{src}"), form).unwrap_or_else(|e| panic!("{name} with BOM: {e:?}")).execution;
        assert_eq!(plain, with_bom, "{name}: a BOM changed the facts");
    }
}

#[test]
fn diagnostics_after_a_bom_point_at_the_same_line_and_column() {
    let bad = "actor Member\nresource Member { fields { id: Idd } }\n";
    let plain = load_str(bad, Form::A).map(|_| ()).unwrap_err();
    let with_bom = load_str(&format!("{BOM}{bad}"), Form::A).map(|_| ()).unwrap_err();
    let at = |ds: &[spike_v1_fixture::diag::Diag]| ds.iter().map(|d| (d.code, d.span.line, d.span.col)).collect::<Vec<_>>();
    assert_eq!(at(&plain), at(&with_bom));
}

#[test]
fn only_one_leading_bom_is_ignored() {
    let src = FORMS[0].2;
    let codes = |s: &str| load_str(s, Form::A).map(|_| vec![]).unwrap_or_else(|ds| ds.iter().map(|d| d.code).collect::<Vec<_>>());
    assert_eq!(codes(&format!("{BOM}{BOM}{src}")), vec!["DEF.LEX_UNEXPECTED_CHAR"], "a second BOM is still a stray character");
    // A BOM inside the text is not a file marker either.
    assert!(load_str(&format!("actor Member{BOM}\n{src}"), Form::A).is_err());
}
