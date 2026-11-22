//! The policy-writing advisories (tenant condition missing, write wider than read, public read of tenant rows) are
//! reported with the development checks off, like the public-count notice, and never change the execution facts.

use serde_json::{json, Value};
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};

const APP: &str = r#"
actor Member
resource Org { fields { id: Id; name: Text(1..40) } rows read when id = actor.org }
resource Member { fields { id: Id; org: Org; name: Text(1..40) } rows read when org = actor.org }
resource Note {
  fields { id: Id; org: Org; body: Text(1..200); published: Bool }
  rows read when org = actor.org
}
"#;

const OPTIONS: [fn() -> Value; 3] = [|| json!({}), || json!({"devChecks": false}), || json!({"devChecks": true})];

fn codes(src: &str, options: &Value) -> Vec<&'static str> {
    load_checked(src, Form::A, options).expect("an advisory never rejects").advisories.iter().map(|a| a.code).collect()
}

#[test]
fn a_tenant_tied_definition_has_no_policy_advice_whatever_the_options() {
    for options in OPTIONS {
        assert!(codes(APP, &options()).is_empty(), "{}", options());
    }
}

#[test]
fn each_policy_advisory_is_reported_without_dev_checks() {
    let unscoped = APP.replace("rows read when org = actor.org\n}", "rows read when published\n}");
    let public = APP.replace("rows read when org = actor.org\n}", "rows read when true\n}");
    let wider = APP.replace("rows read when id = actor.org", "rows read when true").replace(
        "rows read when org = actor.org\n}",
        "rows read when org = actor.org\n  expose create { allow body != \"\"; fields org, body, published }\n}",
    );
    let count = APP.replace(
        "resource Member { fields { id: Id; org: Org; name: Text(1..40) } rows read when org = actor.org }",
        "access anyone(a: Member) = true\nresource Member {\n  fields { id: Id; org: Org; name: Text(1..40) }\n  rows read when org = actor.org\n  aggregate openNotes: Int { source Note; sourceAccess anyone(actor); where published = true; callerFilter none; rowOutput none; release count }\n  expose aggregate openNotes\n}",
    );
    for (src, code) in [
        (&count, "DEF.ADVISORY_TENANT_UNSCOPED_COUNT"),
        (&unscoped, "DEF.ADVISORY_TENANT_UNSCOPED_READ"),
        (&public, "DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD"),
        (&wider, "DEF.ADVISORY_WRITE_WIDER_THAN_READ"),
    ] {
        let mut digests = vec![];
        for options in OPTIONS {
            let checked = load_checked(src, Form::A, &options()).expect("accepted");
            let found: Vec<_> = checked.advisories.iter().map(|a| a.code).collect();
            assert_eq!(found, [code], "{}", options());
            assert!(checked.advisories[0].anchor.starts_with("line:"), "{}", checked.advisories[0].anchor);
            digests.push(digest(&checked.output.execution));
        }
        assert!(digests.windows(2).all(|w| w[0] == w[1]), "the advice does not change the execution digest");
    }
}
