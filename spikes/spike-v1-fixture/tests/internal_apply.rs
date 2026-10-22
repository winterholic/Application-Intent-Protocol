//! `expose apply T { ...; internal }`: the key reaches the facts, and a public `expose compose` cannot name an internal transition.
use spike_v1_fixture::{load_str, Form};

const BASE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Doc {
  fields { id: Id; owner: Member; open: Bool }
  rows read when owner = actor
  transition close { allow owner = actor; from open = true; to open = false }
  EXPOSE
}
"#;

#[test]
fn internal_reaches_the_facts_and_defaults_to_public() {
    let internal = BASE.replace("EXPOSE", "expose apply close { target id; bulk maxRows 1; internal }");
    let facts = load_str(&internal, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    assert_eq!(facts["resources"]["Doc"]["exposeApply"]["close"]["internal"], true);
    let public = BASE.replace("EXPOSE", "expose apply close { target id; bulk maxRows 1 }");
    let facts = load_str(&public, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    assert_eq!(facts["resources"]["Doc"]["exposeApply"]["close"]["internal"], false);
    let twice = BASE.replace("EXPOSE", "expose apply close { target id; bulk maxRows 1; internal; internal }");
    assert!(load_str(&twice, Form::A).is_err(), "a key is written once");
}

#[test]
fn a_public_compose_cannot_name_an_internal_transition() {
    let compose = "expose compose { bulk maxRows 1; transitions close }";
    let ok = BASE.replace("EXPOSE", &format!("expose apply close {{ target id; bulk maxRows 1 }} {compose}"));
    load_str(&ok, Form::A).unwrap_or_else(|e| panic!("a public transition may be composed: {e:?}"));
    let bad = BASE.replace("EXPOSE", &format!("expose apply close {{ target id; bulk maxRows 1; internal }} {compose}"));
    let errors = match load_str(&bad, Form::A) {
        Ok(_) => panic!("compose is a public call and must not reach an internal transition"),
        Err(errors) => errors,
    };
    assert!(errors.iter().any(|error| error.code == "DEF.UNSUPPORTED"), "{errors:?}");
}
