//! Mining job and share types.

use bitcoin::block::Version;
use bitcoin::hash_types::BlockHash;
use bitcoin::pow::{CompactTarget, Target};

use super::extranonce2::Extranonce2;
use super::merkle::MerkleRootKind;
use super::version::VersionTemplate;

/// Represents a mining job from any source.
///
/// Jobs may come from pools (Stratum v1/v2), solo mining, or dummy sources for
/// testing. Depending on the protocol and mode, the merkle root may be fixed or
/// computed dynamically from coinbase transaction parts.
#[derive(Debug, Clone)]
pub struct Job {
    /// Identifier for this job assigned by the source
    pub id: String,

    /// Previous block hash
    pub prev_blockhash: BlockHash,

    /// Block version with optional rolling capability
    pub version: VersionTemplate,

    /// Encoded difficulty target
    pub bits: CompactTarget,

    /// Block timestamp
    pub time: u32,

    /// Specifies how to obtain the merkle root for this job.
    pub merkle_root: MerkleRootKind,
}

impl Job {
    /// Get the target difficulty as a Target type.
    pub fn target(&self) -> Target {
        Target::from(self.bits)
    }
}

/// Represents a share submission (solved work).
#[derive(Debug, Clone)]
pub struct Share {
    /// Job ID this share is for
    pub job_id: String,

    /// Nonce that solves the work
    pub nonce: u32,

    /// Block timestamp
    pub time: u32,

    /// Version bits
    pub version: Version,

    /// Extranonce2
    pub extranonce2: Option<Extranonce2>,
}
