//! V4-2 외부 효과 전달. 쓰기 트랜잭션이 남긴 outbox 행을 별도 소비자가 공급자에게 보낸다.
//! DB 커밋과 외부 효과는 원자적이지 않다. 보장하는 것은 "커밋된 효과만, 최소 한 번 전달 + 공급자 쪽 멱등 키로 한 번 효과"다.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio_postgres::Client;

/// 외부 공급자(메일·푸시 등)의 시험용 대역. 멱등 키로 같은 요청의 두 번째 효과를 버린다.
#[derive(Default)]
pub struct MockProvider {
    pub seen_keys: HashSet<String>,
    pub effects: Vec<(String, i64)>,
    pub calls: usize,
    /// 앞에서부터 이 횟수만큼 실패한다.
    pub fail_first: usize,
    /// 이 수신자는 항상 실패한다(영구 오류).
    pub always_fail_recipient: Option<i64>,
}

impl MockProvider {
    pub fn send(&mut self, key: &str, topic: &str, recipient: i64) -> Result<(), String> {
        self.calls += 1;
        if self.calls <= self.fail_first || self.always_fail_recipient == Some(recipient) {
            return Err("provider unavailable".into());
        }
        // 같은 멱등 키는 성공으로 답하되 효과를 다시 만들지 않는다.
        if self.seen_keys.insert(key.to_string()) {
            self.effects.push((topic.to_string(), recipient));
        }
        Ok(())
    }
}

pub const MAX_ATTEMPTS: i32 = 3;

/// outbox에 전달 상태 열을 더한다(spike: 테스트 schema에서만).
pub async fn prepare(db: &Client, schema: &str) {
    db.batch_execute(&format!(
        "ALTER TABLE {schema}.aip_outbox ADD COLUMN delivered_at timestamptz, ADD COLUMN attempts int NOT NULL DEFAULT 0, ADD COLUMN dead boolean NOT NULL DEFAULT false"
    ))
    .await
    .unwrap();
}

#[derive(Debug, Default, PartialEq)]
pub struct Round {
    pub delivered: usize,
    pub failed: usize,
    pub dead: usize,
}

/// 소비 1회. `crash_after_send`이면 공급자 호출 뒤 전달 기록 전에 중단한 것처럼 rollback한다.
pub async fn consume(db: &mut Client, schema: &str, provider: &Arc<Mutex<MockProvider>>, batch: i64, crash_after_send: bool) -> Round {
    let tx = db.transaction().await.unwrap();
    // SKIP LOCKED: 동시 소비자는 서로 잠근 행을 건너뛴다. 같은 행을 둘이 동시에 보내지 않는다.
    let rows = tx
        .query(
            format!("SELECT id, topic, recipient_id, attempts FROM {schema}.aip_outbox WHERE delivered_at IS NULL AND NOT dead ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED").as_str(),
            &[&batch],
        )
        .await
        .unwrap();
    let mut r = Round::default();
    for row in &rows {
        let (id, topic, recipient, attempts): (i64, String, i64, i32) = (row.get(0), row.get(1), row.get(2), row.get(3));
        // 멱등 키는 outbox 행 id다. 재전달돼도 공급자가 같은 효과로 본다.
        let res = provider.lock().unwrap().send(&format!("outbox-{id}"), &topic, recipient);
        match res {
            Ok(()) => {
                tx.execute(format!("UPDATE {schema}.aip_outbox SET delivered_at = now(), attempts = attempts + 1 WHERE id = $1").as_str(), &[&id])
                    .await
                    .unwrap();
                r.delivered += 1;
            }
            Err(_) if attempts + 1 >= MAX_ATTEMPTS => {
                // 반복 실패는 격리한다. 다른 행 전달을 막지 않게 한다.
                tx.execute(format!("UPDATE {schema}.aip_outbox SET attempts = attempts + 1, dead = true WHERE id = $1").as_str(), &[&id])
                    .await
                    .unwrap();
                r.dead += 1;
            }
            Err(_) => {
                tx.execute(format!("UPDATE {schema}.aip_outbox SET attempts = attempts + 1 WHERE id = $1").as_str(), &[&id]).await.unwrap();
                r.failed += 1;
            }
        }
    }
    if crash_after_send {
        tx.rollback().await.unwrap();
        return Round::default();
    }
    tx.commit().await.unwrap();
    r
}
