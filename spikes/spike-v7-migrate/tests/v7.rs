//! V7 마이그레이션 사전 검사: 같은 데이터에서 계약 변경 9가지를 분류한다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, sqlgen};
use spike_v7_migrate::check;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
fn facts(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}
fn v(old: &str, new: &str) -> Value {
    assert_eq!(A.matches(old).count(), 1, "{old}");
    facts(&A.replacen(old, new, 1))
}
const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 2), (3, NULL);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A', NULL, 1), (11, 'B', NULL, 2), (12, '연합', NULL, NULL);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL), (101, 'A 초안1', '2026-10-10T00:00:00Z', 'DRAFT', 0, 10, NULL),
 (102, 'A 초안2', '2026-10-10T00:00:00Z', 'DRAFT', 0, 10, NULL), (103, 'B 마감', '2026-10-10T00:00:00Z', 'CLOSED', 0, 11, 'x');
";

#[tokio::test(flavor = "current_thread")]
async fn v7_migration_precheck() {
    sqlgen::set_schema("aip_v7");
    let s = sqlgen::schema();
    let base = facts(A);
    let db = connect().await;
    for st in sqlgen::ddl(&base).unwrap() {
        db.batch_execute(&st).await.unwrap();
    }
    db.batch_execute(&SEED.replace("S.", &format!("{s}."))).await.unwrap();
    let club_rows = "  rows read when school = null or school = actor.school\n  expose read { select id, name, logo }";
    // (이름, 새 facts, 기대 분류, 적용 가능)
    let cases: Vec<(&str, Value, &str, bool)> = vec![
        (
            "nullable 필드 추가",
            v(
                "fields { id: Id; name: Text; logo: Url?; school: School? }",
                "fields { id: Id; name: Text; logo: Url?; school: School?; description: Text? }",
            ),
            "Safe",
            true,
        ),
        (
            "필수 필드 추가(기존 행 있음)",
            v("fields { id: Id; name: Text; logo: Url?; school: School? }", "fields { id: Id; name: Text; logo: Url?; school: School?; code: Text }"),
            "Blocked",
            false,
        ),
        (
            "공개 select에서 views 제거",
            v("select id, title, periodEnd, views, bookmarkCount, internalNote", "select id, title, periodEnd, bookmarkCount, internalNote"),
            "Breaking",
            true,
        ),
        // 측정상 확대가 없어도 행 정책 변경은 검토 대상(r8 F8).
        (
            "Club 행 정책 축소",
            v(club_rows, "  rows read when school = actor.school\n  expose read { select id, name, logo }"),
            "SecurityReview",
            false,
        ),
        ("Club 행 정책 확대", v(club_rows, "  rows read when true\n  expose read { select id, name, logo }"), "SecurityReview", false),
        ("internalNote 필드 정책 제거", v("  field internalNote read when managerOf(actor, club)\n", ""), "SecurityReview", false),
        (
            "기존 데이터가 어기는 불변식 추가",
            facts(
                &A.replacen("  invariant atMostOnePublished per club", "  invariant atMostOnePublished per club\n  invariant oneDraft per club", 1)
                    .replacen(
                        "limit atMostOnePublished",
                        "limit oneDraft on Recruitment = atMost 1 where status = DRAFT\nlimit atMostOnePublished",
                        1,
                    ),
            ),
            "Blocked",
            false,
        ),
        // 정의가 쓰는 값(CLOSED) 제거는 V1 의미 검사에서 먼저 거부된다. 여기서는 데이터만 쓰는 값.
        (
            "데이터가 쓰는 enum 값 제거(DRAFT)",
            v("enum RecruitmentStatus { DRAFT, PUBLISHED, CLOSED }", "enum RecruitmentStatus { PUBLISHED, CLOSED }"),
            "Blocked",
            false,
        ),
        (
            "안 쓰이는 enum 값 제거(REJECT)",
            v("enum ApplyStatus { PENDING, APPROVE, REJECT }", "enum ApplyStatus { PENDING, APPROVE }"),
            "Breaking",
            true,
        ),
    ];
    let mut fails = vec![];
    for (name, new, want, want_apply) in &cases {
        let (cs, applicable) = check(&db, &base, new, NOW).await;
        let classes: Vec<&str> = cs.iter().map(|c| c.class).collect();
        let detail: Vec<String> = cs.iter().map(|c| format!("{}[{}] {}", c.what, c.class, c.detail)).collect();
        eprintln!("{name}: {} / 적용 가능 {applicable}", detail.join(" | "));
        if !classes.contains(want) || applicable != *want_apply {
            fails.push(format!("{name}: 기대 {want}/{want_apply}, 실제 {classes:?}/{applicable}"));
        }
    }
    // 측정 수치를 단언한다(r8 F13).
    let detail = |cs: &Vec<spike_v7_migrate::Change>, key: &str| cs.iter().find_map(|c| c.detail.get(key).cloned()).unwrap_or(Value::Null);
    let (c1, _) = check(&db, &base, &cases[1].1, NOW).await;
    let (c3, _) = check(&db, &base, &cases[3].1, NOW).await;
    let (c4, _) = check(&db, &base, &cases[4].1, NOW).await;
    let (c6, _) = check(&db, &base, &cases[6].1, NOW).await;
    let (c7, _) = check(&db, &base, &cases[7].1, NOW).await;
    let (c8, _) = check(&db, &base, &cases[8].1, NOW).await;
    let nums = (
        detail(&c1, "existingRows"),
        detail(&c3, "delta")["lost"].as_array().map(|a| a.len()),
        detail(&c3, "delta")["gained"].as_array().map(|a| a.len()),
        detail(&c4, "delta")["gained"].as_array().map(|a| a.len()),
        detail(&c6, "violatingGroups"),
        detail(&c7, "rowsUsingRemoved"),
        detail(&c8, "rowsUsingRemoved"),
    );
    if nums != (json!(3), Some(4), Some(0), Some(6), json!(1), json!(2), json!(0)) {
        fails.push(format!("측정 수치: {nums:?}"));
    }

    // Codex r8 반례: 분류하지 못한 차이는 모두 자동 적용 불가
    let r8: Vec<(&str, Value, &str)> = vec![
        (
            "predicate 본문 변경",
            v("predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now", "predicate active(r: Recruitment) = true"),
            "SecurityReview",
        ),
        (
            "traverse 변경(관계 공개 필드)",
            facts(&A.replacen("traverse club { select id, name, logo }", "traverse club { select id }", 1)),
            "SecurityReview",
        ),
        (
            "access 본문 변경",
            v("access clubManagerOnly(a: Member, c: Club.Id) = managerOf(a, c)", "access clubManagerOnly(a: Member, c: Club.Id) = a = a and c = c"),
            "SecurityReview",
        ),
        ("전이 allow 확대", v("allow managerOf(actor, club)\n  }", "allow true\n  }"), "SecurityReview"),
        (
            "budget 추가로 루트 조회 공개",
            v(
                "  expose read { select id, name, logo }",
                "  expose read { select id, name, logo; budget { rows 10; depth 1; deadline 2s; cost 100 } }",
            ),
            "SecurityReview",
        ),
        (
            "기존 불변식 조건 변경",
            v(
                "limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED",
                "limit atMostOnePublished on Recruitment = atMost 1 where status = DRAFT",
            ),
            "SecurityReview",
        ),
        ("unique 추가", v("  invariant atMostOnePublished per club", "  invariant atMostOnePublished per club\n  unique club"), "SecurityReview"),
        ("nullable 범위 강화", v("internalNote: Text?", "internalNote: Text(2..100)?"), "Blocked"),
        (
            "새 필드를 쓰는 정책과 필드 동시 추가",
            facts(
                &A.replacen(
                    club_rows,
                    "  rows read when school = null or school = actor.school or canRead = true\n  expose read { select id, name, logo }",
                    1,
                )
                .replacen(
                    "fields { id: Id; name: Text; logo: Url?; school: School? }",
                    "fields { id: Id; name: Text; logo: Url?; school: School?; canRead: Bool? }",
                    1,
                ),
            ),
            "SecurityReview",
        ),
    ];
    // 집행할 수 없는 숫자 조건 불변식은 이제 정의 검사에서 먼저 거부한다. 배포 계획까지 오지 않는다.
    let unsupported = A
        .replacen("  invariant atMostOnePublished per club", "  invariant atMostOnePublished per club\n  invariant twoNonneg per club", 1)
        .replacen("limit atMostOnePublished", "limit twoNonneg on Recruitment = atMost 2 where views >= 0\nlimit atMostOnePublished", 1);
    if !format!("{:?}", load_str(&unsupported, Form::A).err()).contains("UNSUPPORTED_INVARIANT") {
        fails.push("r8 숫자 조건 불변식 추가: 정의 검사에서 UNSUPPORTED_INVARIANT 거부 기대".into());
    }
    for (name, new, want) in &r8 {
        let (cs, applicable) = check(&db, &base, new, NOW).await;
        let classes: Vec<&str> = cs.iter().map(|c| c.class).collect();
        eprintln!(
            "r8 {name}: {classes:?} 적용 {applicable} {}",
            cs.iter().map(|c| c.detail.to_string()).collect::<Vec<_>>().join(" | ").chars().take(160).collect::<String>()
        );
        if !classes.contains(want) || applicable {
            fails.push(format!("r8 {name}: 기대 {want}/불가, 실제 {classes:?}/{applicable}"));
        }
    }

    // 위반 없는 새 불변식은 자동 적용 가능(새 limit 선언만으로 검토로 가지 않음)
    let clean = facts(
        &A.replacen("  invariant atMostOnePublished per club", "  invariant atMostOnePublished per club\n  invariant oneClosed per club", 1)
            .replacen("limit atMostOnePublished", "limit oneClosed on Recruitment = atMost 1 where status = CLOSED\nlimit atMostOnePublished", 1),
    );
    let (cs, applicable) = check(&db, &base, &clean, NOW).await;
    if !applicable || cs.iter().any(|c| c.class != "Safe") {
        fails.push(format!("위반 없는 불변식 추가: {:?}/{applicable}", cs.iter().map(|c| c.class).collect::<Vec<_>>()));
    }

    // 계약 축소의 실제 영향: 옛 화면 요청이 새 계약에서 거부된다
    let narrowed = &cases[2].1;
    let old_req = json!({ "read": "Recruitment", "select": ["id", "views"] });
    let r = plan_read(narrowed, &old_req, &Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }).err().map(|e| e.code);
    if r != Some("FIELD_NOT_EXPOSED") {
        fails.push(format!("옛 요청 거부: {r:?}"));
    }
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
