//! HashTask and Share types for work assignment and result reporting.
//!
//! These are minimal stubs for now - they'll be expanded as the scheduler
//! and job source infrastructure develops.

/// Work assignment from scheduler to hash thread.
///
/// Represents actual mining work from a job source (pool or dummy). The source
/// handle determines whether results are submitted (pool) or discarded (dummy).
///
/// If a thread has no HashTask (None), it's idle (low power, no hashing).
///
/// TODO: Expand with full job template, EN2 range, state for resumption, etc.
#[derive(Debug, Clone)]
pub struct HashTask {
    /// Job identifier for tracking
    pub job_id: u64,

    /// Source ID (0 = dummy/testing, >0 = real pool)
    /// TODO: Replace with SourceHandle when that's available
    pub source_id: u64,
    // Future fields:
    // pub job: Arc<JobTemplate>,
    // pub source_handle: SourceHandle,
    // pub en2_range: Extranonce2Range,
    // pub state: HashTaskState,
}

impl HashTask {
    /// Create a dummy task (full power, results discarded)
    pub fn dummy(job_id: u64) -> Self {
        Self {
            job_id,
            source_id: 0, // Dummy source
        }
    }

    /// Create an active task (real pool work)
    pub fn active(job_id: u64, source_id: u64) -> Self {
        Self { job_id, source_id }
    }

    /// Create a stub task for testing (dummy)
    pub fn stub(job_id: u64) -> Self {
        Self::dummy(job_id)
    }

    /// Check if this is a dummy task (source_id == 0)
    pub fn is_dummy(&self) -> bool {
        self.source_id == 0
    }

    /// Check if this is active pool work
    pub fn is_active(&self) -> bool {
        self.source_id > 0
    }
}

/// Valid share found by a HashThread.
///
/// The hash has already been calculated and filtered by pool_target.
/// TODO: Expand with full block header fields, actual hash, etc.
#[derive(Debug, Clone)]
pub struct Share {
    /// Job this share solves
    pub job_id: u64,

    /// Winning nonce
    pub nonce: u32,
    // Future fields:
    // pub job: Arc<JobTemplate>,
    // pub source_handle: SourceHandle,
    // pub en2: Option<Extranonce2>,
    // pub version: Version,
    // pub ntime: u32,
    // pub hash: BlockHash,
}

impl Share {
    /// Create a stub share for testing
    pub fn stub(job_id: u64, nonce: u32) -> Self {
        Self { job_id, nonce }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_task_dummy() {
        let task = HashTask::dummy(42);
        assert!(task.is_dummy());
        assert!(!task.is_active());
        assert_eq!(task.source_id, 0);
        assert_eq!(task.job_id, 42);
    }

    #[test]
    fn test_hash_task_active() {
        let task = HashTask::active(99, 5);
        assert!(!task.is_dummy());
        assert!(task.is_active());
        assert_eq!(task.source_id, 5);
        assert_eq!(task.job_id, 99);
    }

    #[test]
    fn test_hash_task_stub_creation() {
        let task = HashTask::stub(123);
        assert_eq!(task.job_id, 123);
        assert!(task.is_dummy(), "stub tasks are dummy tasks");
    }

    #[test]
    fn test_share_stub_creation() {
        let share = Share::stub(456, 0xdeadbeef);
        assert_eq!(share.job_id, 456);
        assert_eq!(share.nonce, 0xdeadbeef);
    }

    #[test]
    fn test_hash_task_clone() {
        let task1 = HashTask::stub(789);
        let task2 = task1.clone();
        assert_eq!(task1.job_id, task2.job_id);
        assert_eq!(task1.source_id, task2.source_id);
    }

    #[test]
    fn test_share_clone() {
        let share1 = Share::stub(100, 200);
        let share2 = share1.clone();
        assert_eq!(share1.job_id, share2.job_id);
        assert_eq!(share1.nonce, share2.nonce);
    }
}
