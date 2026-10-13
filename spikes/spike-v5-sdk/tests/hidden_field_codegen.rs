//! Delegated decision CD-3 (2026-10-13, V2-R1): a field hidden by `field x read when cond` is null on rows where the
//! condition is false, so the generated type is `T | null` even for a `NOT NULL` column. The same holds inside a
//! to-one traverse and a 1:N traverse.
use spike_v1_fixture::{load_str, Form};
use spike_v5_sdk::contract_ts;

const DEF: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Id; author: Member; title: Text; secret: Text }
  rows read when true
  field secret read when author = actor
  expose read {
    select id, title, secret
    traverse notes via Note.post { select id, secret; sort id; limit 5 }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
resource Note {
  fields { id: Id; post: Post; owner: Member; secret: Text }
  rows read when true
  field secret read when owner = actor
  expose read {
    select id, secret
    traverse post { select id, title, secret }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
"#;

#[test]
fn a_policy_hidden_required_field_is_nullable_in_the_generated_type() {
    let facts = load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let ts = contract_ts(&facts);
    assert!(ts.contains("secret: string | null;"), "{ts}");
    assert!(!ts.contains("secret: string;"), "no hidden field may be typed non-null:\n{ts}");
    assert!(ts.contains("title: string;"), "an ungated required field stays non-null:\n{ts}");
}
