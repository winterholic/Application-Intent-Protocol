//! V8 인증: 서명 토큰만 actor를 정한다. 위조·변조·만료·다른 키·옛 헤더로는 다른 사람이 될 수 없다.
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, sqlgen};
use spike_v6_transport::auth::{AuthError, Keyring};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  expose read { select id, isChecked; sort id; budget { rows 100; depth 1; deadline 1s; cost 100 } }
}
";

async fn http(port: u16, body: &Value, headers: &str) -> Value {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let b = body.to_string();
    s.write_all(format!("POST /read HTTP/1.1\r\n{headers}content-length: {}\r\n\r\n{b}", b.len()).as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("null")).unwrap_or(Value::Null)
}

#[test]
fn v8_token_unit() {
    let k = Keyring::generate();
    let t = k.issue(7, 60);
    assert_eq!(k.verify(&t), Ok(7));
    // 다른 서버 키로 서명한 토큰
    assert_eq!(Keyring::generate().verify(&t), Err(AuthError::BadSignature));
    // payload의 sub만 바꾼 토큰
    let (_, sig) = t.split_once('.').unwrap();
    let forged = format!("{}.{sig}", B64.encode(json!({ "sub": 1, "iat": 0, "exp": 9_999_999_999i64 }).to_string()));
    assert_eq!(k.verify(&forged), Err(AuthError::BadSignature));
    assert_eq!(k.verify(&k.issue(7, -1)), Err(AuthError::Expired));
    assert_eq!(k.verify("garbage"), Err(AuthError::Malformed));
    assert_eq!(k.verify("a.b!"), Err(AuthError::Malformed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_server_auth() {
    sqlgen::set_schema("aip_v8");
    let s = sqlgen::schema();
    let facts = load_str(&format!("{A}\n{ALARM}"), Form::A).unwrap().execution;
    let db = connect().await;
    for st in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&st).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id, school_id) VALUES (1, NULL), (2, NULL); INSERT INTO {s}.member_alarm (id, member_id, is_checked) VALUES (1, 1, false), (2, 2, false);"
    ))
    .await
    .unwrap();
    let (port, keys) = spike_v6_transport::start(facts).await;
    let q = json!({ "query": { "read": "MemberAlarm", "select": ["id"] } });
    let ids = |v: &Value| v["rows"].as_array().map(|a| a.iter().map(|r| r["id"].as_i64().unwrap()).collect::<Vec<_>>());
    let mut fails = vec![];
    let mut check = |name: &str, got: String, want: &str| {
        if got != want {
            fails.push(format!("{name}: 기대 {want}, 실제 {got}"));
        }
    };
    let t1 = keys.issue(1, 60);
    let authenticated = http(port, &q, &format!("authorization: Bearer {t1}\r\n")).await;
    assert!(authenticated["maxAgeMs"].as_u64().is_some_and(|n| n > 0 && n <= 1000), "검증된 세션의 읽기에는 유한 캐시 수명이 필요: {authenticated}");
    check("정상 토큰", format!("{:?}", ids(&http(port, &q, &format!("authorization: Bearer {t1}\r\n")).await)), "Some([1])");
    check("토큰 없음은 익명", format!("{:?}", ids(&http(port, &q, "").await)), "Some([])");
    let (_, sig) = t1.split_once('.').unwrap();
    let forged = format!("{}.{sig}", B64.encode(json!({ "sub": 2, "iat": 0, "exp": 9_999_999_999i64 }).to_string()));
    check("변조 토큰", http(port, &q, &format!("authorization: Bearer {forged}\r\n")).await["code"].to_string(), "\"UNAUTHENTICATED\"");
    let other = Keyring::generate().issue(2, 60);
    check("다른 키 토큰", http(port, &q, &format!("authorization: Bearer {other}\r\n")).await["code"].to_string(), "\"UNAUTHENTICATED\"");
    let expired = keys.issue(1, -1);
    check("만료 토큰", http(port, &q, &format!("authorization: Bearer {expired}\r\n")).await["code"].to_string(), "\"TOKEN_EXPIRED\"");
    // 실패한 토큰을 익명으로 낮추지 않는다(익명 결과도 주지 않음)
    check("깨진 토큰", http(port, &q, "authorization: Bearer x\r\n").await["code"].to_string(), "\"UNAUTHENTICATED\"");
    // 옛 시험용 헤더로 사칭 불가: 무시되고 익명
    check("옛 x-spike-actor 헤더", format!("{:?}", ids(&http(port, &q, "x-spike-actor: 2\r\n").await)), "Some([])");
    let t2 = keys.issue(2, 60);
    check(
        "authorization 두 번",
        http(port, &q, &format!("authorization: Bearer {t1}\r\nauthorization: Bearer {t2}\r\n")).await["code"].to_string(),
        "\"BAD_REQUEST\"",
    );
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
