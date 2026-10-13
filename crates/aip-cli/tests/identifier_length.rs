//! PostgreSQL cuts identifiers at 63 bytes. Two generated names that share their first 63 bytes then collide
//! (a missing index, or a constraint error that names the wrong thing), so the backend refuses them up front.

const LONG_FIELD: &str = "aVeryLongFieldNameThatKeepsGoingUntilItPassesTheSixtyThreeByteLimitOfPostgresql";

fn compile(source: &str) -> aip_pg::Compiled {
    let checked = aip_sema::pipeline::check_source(source);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    aip_pg::compile(&core, &map)
}

fn app(field: &str) -> String {
    format!(
        "
use auth
actor Member via auth.oidc(kakao)

entity Member {{
  nickname: Text(2..20, trim)
  {field}: Text?
  unique (nickname) else NICKNAME_TAKEN
}}

command Register(nickname: Text(2..20, trim)) idempotent {{
  allow public
  do {{ insert Member {{ nickname }} as m }}
  returns m {{ id nickname }}
}}
"
    )
}

#[test]
fn an_identifier_over_63_bytes_is_an_error_naming_the_cause() {
    let compiled = compile(&app(LONG_FIELD));
    let errors: Vec<_> = compiled.diagnostics.iter().filter(|d| d.is_error()).collect();
    assert!(
        errors.iter().any(|d| d.code == aip_ir::codes::E603 && d.message.contains("63")),
        "a generated name longer than PostgreSQL allows must be rejected: {:?}",
        compiled.diagnostics
    );
}

#[test]
fn names_within_the_limit_still_compile() {
    let compiled = compile(&app("shortField"));
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
}
