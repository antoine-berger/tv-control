use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tv_application::*;
use tv_domain::*;

pub struct SystemClock {
    start: Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
    fn monotonic_ms(&self) -> u64 {
        self.start.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}
#[derive(Default)]
pub struct CancelFlag(pub AtomicBool);
impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Cancellation for CancelFlag {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

struct Entry {
    device: String,
    action: Action,
    receipt: Option<Receipt>,
    completed_at: u64,
    resource: String,
}
pub struct MemoryState {
    entries: Mutex<HashMap<String, Entry>>,
    capacity: usize,
    retention_ms: u64,
    resource_groups: std::collections::BTreeMap<String, String>,
}
impl MemoryState {
    pub fn new(capacity: usize, retention_ms: u64) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            retention_ms,
            resource_groups: Default::default(),
        }
    }
    pub fn with_resource_groups(
        mut self,
        groups: std::collections::BTreeMap<String, String>,
    ) -> Self {
        self.resource_groups = groups;
        self
    }
}
impl StateStore for MemoryState {
    fn reserve(&self, req: &Request, now: u64) -> Reservation {
        let Ok(mut entries) = self.entries.try_lock() else {
            return Reservation::Busy;
        };
        entries.retain(|_, e| {
            e.receipt.is_none() || now.saturating_sub(e.completed_at) < self.retention_ms
        });
        if let Some(e) = entries.get(&req.id) {
            if e.device != req.device || e.action != req.action {
                return Reservation::Conflict;
            }
            return e
                .receipt
                .clone()
                .map(Reservation::Cached)
                .unwrap_or(Reservation::Busy);
        }
        let resource = self.resource_groups.get(&req.device).unwrap_or(&req.device);
        if entries
            .values()
            .any(|e| &e.resource == resource && e.receipt.is_none())
        {
            return Reservation::Busy;
        }
        // Never evict live or retained receipts to make room for a mutation.
        if entries.len() >= self.capacity {
            return Reservation::Full;
        }
        entries.insert(
            req.id.clone(),
            Entry {
                device: req.device.clone(),
                action: req.action.clone(),
                receipt: None,
                completed_at: 0,
                resource: resource.clone(),
            },
        );
        Reservation::Acquired
    }
    fn finish(&self, req: &Request, receipt: &Receipt, now: u64) {
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(e) = entries.get_mut(&req.id) {
                e.receipt = Some(receipt.clone());
                e.completed_at = now;
            }
        }
    }
}
