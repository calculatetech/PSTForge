use libpff_sys::CatalogSink;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProvenance {
    Normal,
    Recovered,
    Orphan,
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCompleteness {
    Complete,
    Partial,
    Damaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    Pending,
    Spooled,
    Written,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ItemKey {
    pub provenance: RecoveryProvenance,
    pub source_node_id: Option<u32>,
    pub recovery_index: Option<u64>,
    pub occurrence: u32,
}

/// Receives one bounded catalog event at a time.
///
/// Durable implementations must return only after the event's candidate
/// transaction is safely recorded. Payload slices never outlive the call.
pub trait CandidateSink: CatalogSink {}

impl<T: CatalogSink + ?Sized> CandidateSink for T {}
