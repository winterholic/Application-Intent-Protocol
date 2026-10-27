//! Product cell-opening boundary for authorized reads.
//!
//! The reader runs its authorization SQL first and calls [`ReadCellOpener`] with the stored
//! frames; [`StoreOpener`] resolves each full frame identity inside its deployment/scope and
//! opens it. There is no plaintext fallback: a non-frame or tampered value fails the read.
use crate::encryption::{CellLocation, DekStore, Error as StoreError, resolve_scope};
use aip_crypto::provider::KekProvider;
use tokio_postgres::Client;

/// One stored frame with its resolved scope and AAD location.
pub struct OpenCell {
    pub scope_kind: String,
    pub scope_id: String,
    pub resource: String,
    pub field: String,
    pub row_id: String,
    pub frame: String,
}

/// Opening failure with a wire-stable code. The reader passes it through unchanged.
pub struct OpenError {
    pub code: &'static str,
    pub msg: String,
}

/// Key boundary called by the reader after its authorization SQL has run.
pub trait ReadCellOpener: Send + Sync {
    /// Opens every cell in order. Reads need no nonce budget; lookups are plain SELECTs.
    fn open_cells<'c>(
        &'c self,
        db: &'c Client,
        cells: Vec<OpenCell>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>, OpenError>> + Send + 'c>>;
}

/// Opens reader cells with a deployment [`DekStore`].
pub struct StoreOpener {
    store: DekStore,
    provider: Box<dyn KekProvider>,
}

impl StoreOpener {
    pub fn new(store: DekStore, provider: impl KekProvider + 'static) -> Self {
        Self { store, provider: Box::new(provider) }
    }

    fn map(error: StoreError, what: &'static str) -> OpenError {
        let (code, msg) = match error {
            StoreError::Crypto | StoreError::Corrupted => ("WIRE.ENCRYPTION_CORRUPTED", format!("{what}: 셀 무결성 확인 실패")),
            StoreError::Database | StoreError::NotFound | StoreError::UsageExhausted | StoreError::StaleWriter => {
                ("WIRE.ENCRYPTION_UNAVAILABLE", format!("{what}: 키를 사용할 수 없음. 잠시 뒤 다시 시도"))
            }
            StoreError::InvalidInput => ("WIRE.INTERNAL", format!("{what} 실패")),
        };
        OpenError { code, msg }
    }
}

impl ReadCellOpener for StoreOpener {
    fn open_cells<'c>(
        &'c self,
        db: &'c Client,
        cells: Vec<OpenCell>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<String>, OpenError>> + Send + 'c>> {
        Box::pin(async move {
            let mut prepared = Vec::with_capacity(cells.len());
            for cell in &cells {
                let scope = resolve_scope(&cell.scope_kind, &cell.scope_id).map_err(|e| Self::map(e, "scope 확인"))?;
                let location = CellLocation::new(&cell.resource, &cell.field, &cell.row_id).map_err(|e| Self::map(e, "셀 위치 확인"))?;
                prepared.push((scope, location));
            }
            let mut out = Vec::with_capacity(cells.len());
            for (index, cell) in cells.iter().enumerate() {
                let (scope, location) = &prepared[index];
                let bytes =
                    self.store.open_cell(db, scope, location, &cell.frame, self.provider.as_ref()).await.map_err(|e| Self::map(e, "셀 열기"))?;
                out.push(String::from_utf8(bytes.to_vec()).map_err(|_| Self::map(StoreError::Corrupted, "셀 열기"))?);
            }
            Ok(out)
        })
    }
}
