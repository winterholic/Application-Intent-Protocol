use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const TEXT: &str = r#"
enum State { PENDING, APPROVED }
enum Role { MEMBER, ADMIN }
resource Member { fields { id: Id; enabled: Bool } }
actor Member
resource Club { fields { id: Id } }
resource ClubMember {
  fields { id: Id; member: Member; club: Club; role: Role }
  unique member, club
  expose create { allow role = MEMBER; fields member, club, role }
}
resource Apply {
  fields { id: Id; member: Member; club: Club; status: State; count: Int }
  transition approve {
    from status = PENDING
    to status = APPROVED, count = count + 1
    allow actor != null
    create ClubMember { member = member; club = club; role = MEMBER }
    update Member where id = member { enabled = true }
    notify member "application.approved"
  }
  expose apply approve { target id; bulk maxRows 20; sameScope club }
  expose compose {
    bulk maxRows 20; sameScope club; transitions approve
    create ClubMember from member, club
    selfRow member by club
  }
}
"#;

fn definition() -> Value {
    json!({
        "enums": {"State": ["PENDING", "APPROVED"], "Role": ["MEMBER", "ADMIN"]}, "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id", "enabled": "Bool"}},
            "Club": {"fields": {"id": "Id"}},
            "ClubMember": {
                "fields": {"id": "Id", "member": "Member", "club": "Club", "role": "Role"},
                "unique": [["member", "club"]],
                "exposeCreate": {"allow": "role = MEMBER", "fields": ["member", "club", "role"]}
            },
            "Apply": {
                "fields": {"id": "Id", "member": "Member", "club": "Club", "status": "State", "count": "Int"},
                "transitions": {"approve": {
                    "from": "status = PENDING", "to": "status = APPROVED, count = count + 1", "allow": "actor != null",
                    "effects": [
                        {"create": "ClubMember", "values": {"member": "member", "club": "club", "role": "MEMBER"}},
                        {"update": "Member", "where": {"id": "member"}, "values": {"enabled": "true"}},
                        {"notify": "member", "topic": "application.approved"}
                    ]
                }},
                "exposeApply": {"approve": {"target": ["id"], "bulkMaxRows": 20, "sameScope": "club"}},
                "exposeCompose": {"bulkMaxRows": 20, "sameScope": "club", "transitions": ["approve"], "creates": {"ClubMember": ["member", "club"]}, "selfRow": "member by club"}
            }
        }
    })
}

fn source(value: &Value, form: Form) -> String {
    match form {
        Form::HTs => format!("import {{ define }} from '@aip/define';\nexport const spec = define({value});"),
        Form::HPy => format!("from aip.define import define\nSPEC = define({value})"),
        _ => panic!("object form"),
    }
}

#[test]
fn existing_write_candidates_have_the_same_facts_in_every_form() {
    let expected = load_str(TEXT, Form::A).unwrap_or_else(|errors| panic!("text: {errors:?}")).execution;
    for form in [Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let actual = match form {
            Form::ETs => load_str(&format!("import {{ aip }} from '@aip/define';\nexport default aip`{TEXT}`;"), form).expect("TS block").execution,
            Form::EPy => load_str(&format!("from aip.define import aip\nSPEC = aip('''{TEXT}''')"), form).expect("Python block").execution,
            _ => load_str(&source(&definition(), form), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution,
        };
        assert_eq!(actual, expected, "{form:?}");
    }
}

#[test]
fn host_write_shapes_reject_ambiguous_effects_and_unknown_options() {
    let cases = [
        ("/resources/Apply/transitions/approve/effects/0", json!({"create": "ClubMember", "notify": "member", "values": {}}), "H_SHAPE"),
        ("/resources/Apply/transitions/approve/effects/1/where", json!("id = member"), "H_SHAPE"),
        ("/resources/Apply/exposeCompose/selfRow", json!("member by club extra"), "H_SHAPE"),
        ("/resources/Apply/exposeCompose/creates", json!({"ClubMember": "member"}), "H_SHAPE"),
        ("/resources/Apply/exposeCompose/creates", json!({"ClubMember": []}), "H_SHAPE"),
        ("/resources/ClubMember/exposeCreate", json!({"allow": "true", "fields": ["member", "club", "role"], "unknown": 1}), "UNKNOWN_KEY"),
    ];
    for form in [Form::HTs, Form::HPy] {
        for (pointer, bad, code) in &cases {
            let mut value = definition();
            *value.pointer_mut(pointer).expect("mutation target") = bad.clone();
            let errors = load_str(&source(&value, form), form).err().expect("invalid write declaration");
            assert!(errors.iter().any(|error| error.code == *code), "{form:?} {pointer}: {errors:?}");
        }
    }
}
