//! Product cell-sealing boundary for the native writer.
//!
//! The writer resolves scope and AAD location after authorization and calls
//! [`WriteCellSealer`]; [`StoreSealer`] validates the resolved request again, reserves nonce
//! budget on its own connection, and seals each cell inside the resource transaction.
//! Failures carry wire-stable codes without key, plaintext, or ciphertext detail.
use crate::encryption::{CellLocation, DekStore, Error as StoreError, Scope, SealReservation};
use aip_crypto::cell::MAX_CELL_BYTES;
use aip_crypto::provider::KekProvider;
use std::collections::BTreeMap;
use tokio::sync::Mutex;
use tokio_postgres::{Client, Transaction};

/// One validated plaintext cell with its resolved scope and AAD location.
pub struct SealCell {
    pub scope_kind: String,
    pub scope_id: String,
    pub resource: String,
    pub field: String,
    pub row_id: String,
    pub plaintext: Vec<u8>,
}

/// Sealing failure with a wire-stable code. The writer passes it through unchanged.
pub struct SealError {
    pub code: &'static str,
    pub msg: String,
}

/// Key boundary called by the native writer after authorization. Reservations are committed
/// on the implementation's own budget connection; sealing runs in the resource transaction.
pub trait WriteCellSealer: Send + Sync {
    fn seal_cells<'c>(
        &'c self,
        tx: &'c Transaction<'c>,
        cells: Vec<SealCell>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>, SealError>> + Send + 'c>>;
}

/// Seals writer cells with a deployment [`DekStore`]. `budget` is a dedicated, otherwise-idle
/// connection used only for usage reservations; it must differ from the resource connection.
pub struct StoreSealer {
    store: DekStore,
    provider: Box<dyn KekProvider>,
    budget: Mutex<Client>,
}

impl StoreSealer {
    pub fn new(store: DekStore, provider: impl KekProvider + 'static, budget: Client) -> Self {
        Self { store, provider: Box::new(provider), budget: Mutex::new(budget) }
    }

    fn scope(kind: &str, id: &str) -> Result<Scope, StoreError> {
        if kind == "service" {
            if id == "global" {
                return Ok(Scope::service());
            }
            return Err(StoreError::InvalidInput);
        }
        let (kind, resource) = kind.split_once(':').ok_or(StoreError::InvalidInput)?;
        match kind {
            "ref" => Scope::reference(resource, id),
            "owner" => Scope::owner(resource, id),
            _ => Err(StoreError::InvalidInput),
        }
    }

    fn map(error: StoreError, what: &'static str) -> SealError {
        let (code, msg) = match error {
            StoreError::Crypto | StoreError::Corrupted => ("WIRE.ENCRYPTION_CORRUPTED", format!("{what}: 셀 무결성 확인 실패")),
            StoreError::Database | StoreError::NotFound | StoreError::UsageExhausted | StoreError::StaleWriter => {
                ("WIRE.ENCRYPTION_UNAVAILABLE", format!("{what}: 키를 사용할 수 없음. 잠시 뒤 다시 시도"))
            }
            StoreError::InvalidInput => ("WIRE.INTERNAL", format!("{what} 실패")),
        };
        SealError { code, msg }
    }
}

impl WriteCellSealer for StoreSealer {
    fn seal_cells<'c>(
        &'c self,
        tx: &'c Transaction<'c>,
        cells: Vec<SealCell>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>, SealError>> + Send + 'c>> {
        Box::pin(async move {
            if cells.is_empty() {
                return Ok(vec![]);
            }
            // Validate every cell before reserving anything: a reservation commits budget
            // that rollback cannot replenish, so invalid input must fail before that point.
            let mut prepared = Vec::with_capacity(cells.len());
            for cell in &cells {
                if cell.plaintext.len() > MAX_CELL_BYTES {
                    return Err(SealError {
                        code: "WIRE.BAD_VALUE", msg: format!("`{}.{}` 셀이 크기 상한을 넘음", cell.resource, cell.field)
                    });
                }
                let scope = Self::scope(&cell.scope_kind, &cell.scope_id).map_err(|e| Self::map(e, "scope 확인"))?;
                let location = CellLocation::new(&cell.resource, &cell.field, &cell.row_id).map_err(|e| Self::map(e, "셀 위치 확인"))?;
                prepared.push((scope, location));
            }
            // One reservation per scope, in deterministic order. Scopes are independent keys,
            // so grouping keeps each reservation under the per-call ceiling.
            let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
            for (index, cell) in cells.iter().enumerate() {
                groups.entry((cell.scope_kind.clone(), cell.scope_id.clone())).or_default().push(index);
            }
            let mut reservations: BTreeMap<(String, String), SealReservation> = BTreeMap::new();
            {
                let mut budget = self.budget.lock().await;
                for (key, members) in &groups {
                    let (scope, _) = &prepared[members[0]];
                    let reservation =
                        self.store.reserve_for_write(&mut budget, scope, members.len() as u32).await.map_err(|e| Self::map(e, "사용량 예약"))?;
                    reservations.insert(key.clone(), reservation);
                }
            }
            let mut frames = vec![String::new(); cells.len()];
            for (index, cell) in cells.iter().enumerate() {
                let key = (cell.scope_kind.clone(), cell.scope_id.clone());
                let reservation = reservations.get_mut(&key).ok_or_else(|| Self::map(StoreError::Corrupted, "사용량 예약"))?;
                let (_, location) = &prepared[index];
                frames[index] = self
                    .store
                    .seal_in(tx, reservation, location, &cell.plaintext, self.provider.as_ref())
                    .await
                    .map_err(|e| Self::map(e, "셀 봉인"))?;
            }
            Ok(frames)
        })
    }
}
