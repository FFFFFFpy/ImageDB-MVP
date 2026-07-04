//! Failure injection infrastructure for the commit pipeline.
//!
//! This module provides fault injection points that can be triggered
//! during commit operations to test recovery paths. The actual test
//! functions are in the `tests` submodule (only compiled in test mode).
#![allow(dead_code)]
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Fault injection points in the commit pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitFaultPoint {
    AfterDbWrite = 0,
    DuringCopy = 1,
    AfterStagingCopy = 2,
    AfterStagingVerify = 3,
    AfterManifestWrite = 4,
    BeforePublishRename = 5,
    AfterPublishRename = 6,
    BeforeDbCommit = 7,
    AfterDbCommit = 8,
    BeforeSourceArchive = 9,
    DuringSourceArchive = 10,
    BeforeCommitMarker = 11,
    Panic = 12,
}

/// Global fault injection state (thread-safe).
static FAULT_POINT: AtomicU8 = AtomicU8::new(255);
static FAULT_COUNTER: AtomicUsize = AtomicUsize::new(0);
static FORCE_CONSERVATIVE_PUBLISH: AtomicU8 = AtomicU8::new(0);

/// Set the active fault point for the next commit operation.
pub(crate) fn set_fault_point(fault: CommitFaultPoint) {
    FAULT_COUNTER.store(0, Ordering::SeqCst);
    FAULT_POINT.store(fault as u8, Ordering::SeqCst);
}

/// Clear the active fault point.
pub(crate) fn clear_fault_point() {
    FAULT_POINT.store(255, Ordering::SeqCst);
}

pub(crate) fn set_force_conservative_publish(enabled: bool) {
    FORCE_CONSERVATIVE_PUBLISH.store(u8::from(enabled), Ordering::SeqCst);
}

pub(crate) fn force_conservative_publish() -> bool {
    FORCE_CONSERVATIVE_PUBLISH.load(Ordering::SeqCst) == 1
}

/// Check if a fault should be triggered at the given point.
pub(crate) fn check_fault(point: CommitFaultPoint) -> bool {
    let current = FAULT_POINT.load(Ordering::SeqCst);
    if current == 255 {
        return false;
    }
    if current == point as u8 {
        let _ = FAULT_COUNTER.fetch_add(1, Ordering::SeqCst);
        true
    } else {
        false
    }
}

/// Check fault and return an error if triggered.
pub(crate) fn maybe_fault(
    point: CommitFaultPoint,
    msg: &str,
) -> Result<(), crate::error::AppError> {
    if check_fault(point) {
        Err(crate::error::AppError::Internal(format!(
            "injected fault at {:?}: {msg}",
            point
        )))
    } else {
        Ok(())
    }
}
