//! HashThread abstraction for schedulable mining workers.
//!
//! A HashThread represents a schedulable group of hashing engines that work
//! together to execute mining tasks. The scheduler assigns work to HashThreads
//! without needing to know about the underlying hardware topology (single chip,
//! chip chain, engine groups, etc.).
//!
//! HashThreads are autonomous actors that self-manage their hardware, filter
//! shares, and report events back to the scheduler.

pub mod bm13xx;
pub mod task;

use async_trait::async_trait;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use task::{HashTask, Share};

/// Thread removal signal sent via watch channel.
///
/// Boards monitor hardware and send removal signals to threads when they need
/// to shut down. The signal starts as `Running` and changes to a specific
/// removal reason when shutdown is needed.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadRemovalSignal {
    /// Thread should continue running normally
    Running,

    /// Remove: Board was unplugged from USB
    BoardDisconnected,

    /// Remove: Board detected hardware fault (overheating, power issue, etc.)
    HardwareFault { description: String },

    /// Remove: User requested board disable via API
    UserRequested,

    /// Remove: Graceful system shutdown
    Shutdown,
}

/// HashThread identity based on Tokio task ID.
///
/// Each HashThread runs as an independent Tokio task. The thread's identity
/// is derived from its task ID, providing:
/// - Natural uniqueness (task IDs are unique while task is alive)
/// - Cheap cloning (Arc-wrapped for sharing)
/// - HashMap compatibility (implements Hash + Eq)
/// - No central registry needed
///
/// Note: Task IDs may be recycled after a task exits and all handles are dropped.
/// This is acceptable since we remove threads from the scheduler's registry when
/// they go offline.
#[derive(Clone)]
pub struct ThreadId(Arc<tokio::task::Id>);

impl ThreadId {
    /// Create ThreadId from a Tokio task handle
    ///
    /// This is the canonical way to create a ThreadId - from the task that
    /// will run the thread's actor loop.
    pub(crate) fn from_task<T>(handle: &JoinHandle<T>) -> Self {
        Self(Arc::new(handle.id()))
    }
}

/// Identity based on task ID equality
impl PartialEq for ThreadId {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

impl Eq for ThreadId {}

/// Hash based on task ID
impl Hash for ThreadId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl std::fmt::Debug for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThreadId({:?})", self.0)
    }
}

/// HashThread capabilities reported to scheduler for work assignment decisions.
#[derive(Debug, Clone)]
pub struct HashThreadCapabilities {
    /// Estimated hashrate in H/s
    pub hashrate_estimate: f64,
    // Future capabilities:
    // pub can_roll_version: bool,
    // pub version_roll_bits: u32,
    // pub can_roll_ntime: bool,
    // pub ntime_range: Option<std::ops::Range<u32>>,
    // pub can_iterate_extranonce2: bool,
}

/// Current runtime status of a HashThread.
#[derive(Debug, Clone, Default)]
pub struct HashThreadStatus {
    /// Current hashrate estimate in H/s
    pub hashrate: f64,

    /// Number of shares found (at chip target level, before pool filtering)
    pub chip_shares_found: u64,

    /// Number of shares submitted to pool (after filtering)
    pub pool_shares_submitted: u64,

    /// Number of hardware errors detected
    pub hardware_errors: u64,

    /// Current chip temperature if available
    pub temperature_c: Option<f32>,

    /// Whether thread is actively working
    pub is_active: bool,
}

/// Events emitted by HashThreads back to the scheduler.
#[derive(Debug)]
pub enum HashThreadEvent {
    /// Valid share found (already filtered by pool_target)
    ShareFound(Share),

    /// Work approaching exhaustion (warning to scheduler)
    WorkDepletionWarning {
        /// Estimated remaining time in milliseconds
        estimated_remaining_ms: u64,
    },

    /// Work completely exhausted
    WorkExhausted {
        /// Number of EN2 values searched
        en2_searched: u64,
    },

    /// Periodic status update
    StatusUpdate(HashThreadStatus),

    /// Thread going offline (any reason: USB unplug, fault, user request, shutdown)
    GoingOffline,
}

/// Error types for HashThread operations.
#[derive(Debug, thiserror::Error)]
pub enum HashThreadError {
    #[error("Thread has been shut down")]
    ThreadOffline,

    #[error("Channel closed: {0}")]
    ChannelClosed(String),

    #[error("Work assignment failed: {0}")]
    WorkAssignmentFailed(String),

    #[error("Preemption failed: {0}")]
    PreemptionFailed(String),

    #[error("Shutdown timeout")]
    ShutdownTimeout,
}

/// HashThread trait - the scheduler's view of a schedulable worker.
///
/// A HashThread represents a group of hashing engines that can be assigned work
/// as a unit. The scheduler interacts with threads through this trait without
/// needing to know about the underlying hardware topology.
///
/// Threads are autonomous actors that:
/// - Monitor their hardware
/// - Filter shares by pool target
/// - Self-tune chip targets
/// - Report events asynchronously
#[async_trait]
pub trait HashThread: Send {
    /// Get unique thread identifier
    fn id(&self) -> ThreadId;

    /// Get thread capabilities for scheduling decisions
    fn capabilities(&self) -> &HashThreadCapabilities;

    /// Update current work (shares from old work still valid)
    ///
    /// Thread continues hashing old work until new work is ready. Late-arriving
    /// shares from the old work can still be submitted (they're valuable).
    /// Returns the old task for potential resumption (None if thread was idle).
    ///
    /// Used when pool sends updated job (difficulty change, new transactions in
    /// mempool) but the work is fundamentally still valid.
    async fn update_work(
        &mut self,
        new_work: HashTask,
    ) -> std::result::Result<Option<HashTask>, HashThreadError>;

    /// Replace current work (old work invalidated)
    ///
    /// Old work is immediately invalid - discard it and don't submit shares
    /// from it. Returns the old task for tracking purposes (None if thread was
    /// idle).
    ///
    /// Used when blockchain tip changes (new prevhash) or pool signals
    /// clean_jobs.
    async fn replace_work(
        &mut self,
        new_work: HashTask,
    ) -> std::result::Result<Option<HashTask>, HashThreadError>;

    /// Put thread in idle state (low power, no hashing)
    ///
    /// Returns the current task if thread was working (None if already idle).
    /// Thread enters low-power mode, stops hashing.
    async fn go_idle(&mut self) -> std::result::Result<Option<HashTask>, HashThreadError>;

    /// Take ownership of the event receiver for this thread
    ///
    /// Called once by scheduler after thread creation. The scheduler uses this
    /// to receive events (shares, status updates, etc.) from the thread.
    fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<HashThreadEvent>>;

    /// Get current runtime status
    ///
    /// This is cached and may be slightly stale (updated periodically by
    /// thread's status updates).
    fn status(&self) -> HashThreadStatus;

    /// Shutdown the thread gracefully
    ///
    /// Waits for the thread's actor task to exit. Thread will send GoingOffline
    /// event before shutting down.
    async fn shutdown(&mut self) -> std::result::Result<(), HashThreadError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    // Helper to create test thread IDs from dummy tasks
    fn make_test_id() -> ThreadId {
        let handle = tokio::spawn(async {});
        ThreadId::from_task(&handle)
    }

    #[tokio::test]
    async fn test_thread_id_equality_same_task() {
        let id1 = make_test_id();
        let id2 = id1.clone();

        assert_eq!(id1, id2, "Cloned ThreadIds should be equal");
    }

    #[tokio::test]
    async fn test_thread_id_inequality_different_tasks() {
        let id1 = make_test_id();
        let id2 = make_test_id();

        assert_ne!(
            id1, id2,
            "Different ThreadIds (from different tasks) should not be equal"
        );
    }

    #[tokio::test]
    async fn test_thread_id_hash_consistency() {
        use std::collections::hash_map::DefaultHasher;

        let id = make_test_id();
        let id_clone = id.clone();

        let mut hasher1 = DefaultHasher::new();
        id.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        id_clone.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2, "Same ThreadId should hash consistently");
    }

    #[tokio::test]
    async fn test_thread_id_different_hashes() {
        use std::collections::hash_map::DefaultHasher;

        let id1 = make_test_id();
        let id2 = make_test_id();

        let mut hasher1 = DefaultHasher::new();
        id1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        id2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        // Tokio task IDs are distinct, so hashes should be different
        assert_ne!(
            hash1, hash2,
            "Different ThreadIds should have different hashes"
        );
    }

    #[tokio::test]
    async fn test_thread_id_in_hashmap() {
        let id1 = make_test_id();
        let id2 = make_test_id();
        let id3 = id1.clone();

        let mut map = HashMap::new();
        map.insert(id1.clone(), "thread1");
        map.insert(id2.clone(), "thread2");

        assert_eq!(map.get(&id1), Some(&"thread1"));
        assert_eq!(map.get(&id2), Some(&"thread2"));
        assert_eq!(
            map.get(&id3),
            Some(&"thread1"),
            "Cloned ID should map to same value"
        );
        assert_eq!(map.len(), 2, "Should only have two entries");
    }

    #[tokio::test]
    async fn test_thread_id_in_hashset() {
        let id1 = make_test_id();
        let id2 = make_test_id();
        let id3 = id1.clone();

        let mut set = HashSet::new();
        set.insert(id1.clone());
        set.insert(id2.clone());
        set.insert(id3); // Clone of id1

        assert_eq!(set.len(), 2, "Set should contain only unique IDs");
        assert!(set.contains(&id1));
        assert!(set.contains(&id2));
    }

    #[tokio::test]
    async fn test_thread_id_debug_format() {
        let id = make_test_id();
        let debug_str = format!("{:?}", id);

        // Should show Tokio task ID
        assert!(
            debug_str.starts_with("ThreadId(Id("),
            "Debug format should show Tokio task ID: {}",
            debug_str
        );
    }
}
