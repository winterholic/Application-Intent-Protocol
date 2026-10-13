//! Per-principal ceilings on top of the process-wide slot semaphores (O-05).
//!
//! The semaphores bound the whole server, which alone lets one authenticated principal hold every realtime connection,
//! worker or upload slot. A permit here is taken together with the global slot, so a principal can exhaust only its own share.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(crate) struct PrincipalSlots {
    max: usize,
    held: Mutex<HashMap<String, usize>>,
}

pub(crate) struct PrincipalPermit {
    slots: Arc<PrincipalSlots>,
    principal: String,
}

impl PrincipalSlots {
    pub(crate) fn new(max: usize) -> Arc<Self> {
        Arc::new(Self { max, held: Mutex::default() })
    }

    /// `None` when `principal` already holds its share. Entries are removed when their last permit drops, so the map
    /// never holds more principals than there are live permits.
    pub(crate) fn try_acquire(self: &Arc<Self>, principal: &str) -> Option<PrincipalPermit> {
        let mut held = self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = held.get(principal).copied().unwrap_or(0);
        if count >= self.max {
            return None;
        }
        held.insert(principal.to_owned(), count + 1);
        Some(PrincipalPermit { slots: self.clone(), principal: principal.to_owned() })
    }
}

impl Drop for PrincipalPermit {
    fn drop(&mut self) {
        let mut held = self.slots.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = held.get_mut(&self.principal) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.principal);
            }
        }
    }
}

/// The three slot classes that are shared across principals.
pub(crate) struct PrincipalSlotSet {
    pub workers: Arc<PrincipalSlots>,
    pub uploads: Arc<PrincipalSlots>,
    pub realtime: Arc<PrincipalSlots>,
}

impl PrincipalSlotSet {
    /// For constructors that run without per-principal policy (the prototype listeners).
    pub(crate) fn unlimited() -> Self {
        Self { workers: PrincipalSlots::new(usize::MAX), uploads: PrincipalSlots::new(usize::MAX), realtime: PrincipalSlots::new(usize::MAX) }
    }

    pub(crate) fn from_limits(limits: &crate::infrastructure::ResourceLimits) -> Self {
        Self {
            workers: PrincipalSlots::new(limits.per_principal_workers),
            uploads: PrincipalSlots::new(limits.per_principal_uploads),
            realtime: PrincipalSlots::new(limits.per_principal_realtime),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_principal_cannot_exceed_its_share_and_permits_return_on_drop() {
        let slots = PrincipalSlots::new(2);
        let first = slots.try_acquire("a").unwrap();
        let _second = slots.try_acquire("a").unwrap();
        assert!(slots.try_acquire("a").is_none());
        assert!(slots.try_acquire("b").is_some(), "another principal keeps its own share");
        drop(first);
        assert!(slots.try_acquire("a").is_some());
    }

    #[test]
    fn idle_principals_leave_no_entries_behind() {
        let slots = PrincipalSlots::new(1);
        for index in 0..100 {
            drop(slots.try_acquire(&format!("p{index}")).unwrap());
        }
        assert!(slots.held.lock().unwrap().is_empty());
    }
}
