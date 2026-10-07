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

fn with_scalars(form: Form) -> String {
    let (source, field, email) = match form {
        Form::A => (A, "periodEnd: Time", "internalNote: Text?"),
        Form::ETs => (ETS, "periodEnd: Time", "internalNote: Text?"),
        Form::EPy => (EPY, "periodEnd: Time", "internalNote: Text?"),
        Form::HTs => (HTS, "periodEnd: \"Time\"", "internalNote: \"Text?\""),
        Form::HPy => (HPY, "\"periodEnd\": \"Time\"", "\"internalNote\": \"Text?\""),
    };
    let source = replace_once(source, field, &field.replace("Time", "Date"), form);
    let source = replace_once(&source, email, &email.replace("Text", "Email"), form);
    replace_once(&source, "r.status = PUBLISHED and r.periodEnd >= now", "r.status = PUBLISHED", form)
}

#[test]
fn email_and_date_lower_to_the_same_typed_facts_in_all_five_definition_forms() {
    let forms = [Form::A, Form::ETs, Form::EPy, Form::HTs, Form::HPy];
    let mut execution_digests = Vec::new();
    for form in forms {
        let output = load_str(&with_scalars(form), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}"));
        let fields = &output.execution["resources"]["Recruitment"]["fields"];
        assert_eq!(fields["internalNote"]["ty"], "Email?", "{form:?}");
        assert_eq!(fields["periodEnd"]["ty"], "Date", "{form:?}");
        execution_digests.push(digest(&output.execution));
    }
    assert!(execution_digests.iter().all(|digest| digest == &execution_digests[0]));
}

#[test]
fn email_and_date_string_literals_keep_their_contextual_types() {
    let source = replace_once(A, "title: Text(1..100)", "title: Email", Form::A);
    let source = replace_once(&source, "periodEnd: Time", "periodEnd: Date", Form::A);
    let source = replace_once(&source, "r.status = PUBLISHED and r.periodEnd >= now", "r.status = PUBLISHED", Form::A);
    let source =
        replace_once(&source, "to status = CLOSED", "to status = CLOSED, title = \"person@example.test\", periodEnd = \"2024-02-29\"", Form::A);
    let output = load_str(&source, Form::A).unwrap();
    let to = &output.execution["resources"]["Recruitment"]["transitions"]["close"]["to"];
    assert_eq!(to["title"], serde_json::json!({"lit":"person@example.test","ty":"Email"}));
    assert_eq!(to["periodEnd"], serde_json::json!({"lit":"2024-02-29","ty":"Date"}));

    for invalid_date in ["0000-01-01", "2026-02-29", "2026-2-9", "2026-02-２９", "10000-01-01"] {
        let invalid = source.replace("2024-02-29", invalid_date);
        match load_str(&invalid, Form::A) {
            Err(diagnostics) => assert!(diagnostics.iter().any(|diag| diag.code == "BAD_VALUE"), "{invalid_date}: {diagnostics:?}"),
            Ok(_) => panic!("invalid Date literal `{invalid_date}` was accepted"),
        }
    }
    let invalid_email = replace_once(&source, "person@example.test", "not-an-email", Form::A);
    assert!(matches!(load_str(&invalid_email, Form::A), Err(errors) if errors.iter().any(|diag| diag.code == "BAD_VALUE")));
}
