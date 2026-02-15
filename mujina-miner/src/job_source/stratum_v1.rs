//! Stratum v1 job source implementation.
//!
//! This module integrates the Stratum v1 client into mujina-miner's job source
//! abstraction. It handles the conversion between Stratum protocol messages and
//! the internal JobTemplate/Share types used by the scheduler.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::stratum_v1::{ClientCommand, ClientEvent, JobNotification, PoolConfig};
use crate::tracing::prelude::*;
use crate::types::{Difficulty, HashRate, ShareRate, target_for_share_rate};

use super::{
    Extranonce2Range, GeneralPurposeBits, JobTemplate, MerkleRootKind, MerkleRootTemplate, Share,
    SourceCommand, SourceEvent, VersionTemplate,
};

/// Target share rate for suggest_difficulty: 20 shares/min (one every 3 sec).
const SUGGESTED_SHARE_RATE: ShareRate = ShareRate::from_interval(Duration::from_secs(3));

/// Re-suggest when new difficulty is >2x or <0.5x the last-suggested value.
const MATERIAL_CHANGE_FACTOR: f64 = 2.0;

/// Exponential backoff for reconnection timing.
///
/// Starts at `initial` and doubles after each call to `next_delay()`,
/// capping at `max`. Each returned delay is jittered to [0.5, 1.0) of
/// the nominal value to avoid thundering-herd reconnections.
struct ExponentialBackoff {
    current: Duration,
    initial: Duration,
    max: Duration,
    // Per-process jitter seed. RandomState is seeded from OS randomness
    // at construction, so different processes produce different jitter
    // even when reconnecting at the same wall-clock instant. This is
    // the same approach tokio uses internally for jittered timeouts.
    jitter_state: RandomState,
    jitter_step: u64,
}

#[expect(dead_code, reason = "used by reconnection loop in next commit")]
impl ExponentialBackoff {
    fn new(initial: Duration, max: Duration) -> Self {
        Self {
            current: initial,
            initial,
            max,
            jitter_state: RandomState::new(),
            jitter_step: 0,
        }
    }

    /// Return the next backoff delay (with jitter) and advance the state.
    ///
    /// The nominal delay (1s, 2s, 4s, ...) is scaled by a jitter factor
    /// in [0.5, 1.0] to spread out reconnection attempts across miners.
    fn next_delay(&mut self) -> Duration {
        let nominal = self.current;
        self.current = (self.current * 2).min(self.max);

        let mut hasher = self.jitter_state.build_hasher();
        hasher.write_u64(self.jitter_step);
        self.jitter_step = self.jitter_step.wrapping_add(1);
        let hash = hasher.finish();
        let jitter = 0.5 + (hash as f64 / u64::MAX as f64) * 0.5;

        nominal.mul_f64(jitter)
    }

    /// Reset backoff to the initial delay.
    fn reset(&mut self) {
        self.current = self.initial;
    }
}

/// Stratum v1 job source.
///
/// Wraps a StratumV1Client and bridges between the Stratum protocol and
/// mujina-miner's job source abstraction. Converts incoming mining.notify
/// messages to JobTemplates and outgoing Share submissions to Stratum format.
pub struct StratumV1Source {
    /// Pool configuration
    config: PoolConfig,

    /// Where to send events to scheduler
    event_tx: mpsc::Sender<SourceEvent>,

    /// Where to receive commands from scheduler
    command_rx: mpsc::Receiver<SourceCommand>,

    /// Shutdown signal
    shutdown: CancellationToken,

    /// Protocol state from subscription
    state: Option<ProtocolState>,

    /// Track if first accepted share has been logged
    first_share_logged: bool,

    /// Expected hashrate (an estimate, not a measurement)
    expected_hashrate: HashRate,

    /// Last difficulty we suggested to the pool (for material-change detection)
    last_suggested_difficulty: Option<u64>,
}

/// Protocol state after successful subscription.
#[derive(Debug, Clone)]
struct ProtocolState {
    /// Extranonce1 from mining.subscribe
    extranonce1: Vec<u8>,

    /// Extranonce2 size in bytes
    extranonce2_size: usize,

    /// Current share difficulty (from mining.set_difficulty)
    share_difficulty: Option<Difficulty>,

    /// Authorized version mask (from mining.configure or mining.set_version_mask)
    version_mask: Option<u32>,
}

impl StratumV1Source {
    /// Create a new Stratum v1 source.
    pub fn new(
        config: PoolConfig,
        command_rx: mpsc::Receiver<SourceCommand>,
        event_tx: mpsc::Sender<SourceEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            config,
            event_tx,
            command_rx,
            shutdown,
            state: None,
            first_share_logged: false,
            expected_hashrate: HashRate::default(),
            last_suggested_difficulty: None,
        }
    }

    /// Human-readable name derived from pool URL (e.g., "solo.ckpool.org:3333").
    pub fn name(&self) -> String {
        self.config
            .url
            .strip_prefix("stratum+tcp://")
            .or_else(|| self.config.url.strip_prefix("stratum://"))
            .or_else(|| self.config.url.strip_prefix("tcp://"))
            .unwrap_or(&self.config.url)
            .to_string()
    }

    /// Convert Stratum JobNotification to JobTemplate.
    fn job_to_template(&self, job: JobNotification) -> Result<JobTemplate> {
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No protocol state (not subscribed)"))?;

        // Create extranonce2 range (full range for the given size)
        let extranonce2_range = Extranonce2Range::new(state.extranonce2_size as u8)?;

        // Convert version to VersionTemplate
        // Use authorized mask from pool (or none if pool didn't authorize version rolling)
        let gp_bits_mask = state
            .version_mask
            .map(|mask| GeneralPurposeBits::from(&mask.to_be_bytes()))
            .unwrap_or_else(GeneralPurposeBits::none);

        let version_template = VersionTemplate::new(job.version, gp_bits_mask)?;

        // Use pool's share difficulty directly (scheduler handles rate limiting)
        let share_difficulty = state.share_difficulty.unwrap_or(Difficulty::from(1));
        let share_target = share_difficulty.to_target();

        Ok(JobTemplate {
            id: job.job_id,
            prev_blockhash: job.prev_hash,
            version: version_template,
            bits: job.nbits,
            share_target,
            time: job.ntime,
            merkle_root: MerkleRootKind::Computed(MerkleRootTemplate {
                coinbase1: job.coinbase1,
                extranonce1: state.extranonce1.clone(),
                extranonce2_range,
                coinbase2: job.coinbase2,
                merkle_branches: job.merkle_branches,
            }),
        })
    }

    /// Handle a client event.
    async fn handle_client_event(&mut self, event: ClientEvent) -> Result<()> {
        match event {
            ClientEvent::VersionRollingConfigured { authorized_mask } => {
                if let Some(mask) = authorized_mask {
                    debug!(
                        mask = format!("{:#x}", mask),
                        "Version rolling authorized by pool"
                    );
                } else {
                    debug!("Pool doesn't support version rolling");
                }

                // Store the mask (or lack thereof)
                if let Some(state) = &mut self.state {
                    state.version_mask = authorized_mask;
                } else {
                    // Configure happens before subscribe, so state might not exist yet
                    // Create temporary state that will be updated by Subscribed event
                    self.state = Some(ProtocolState {
                        extranonce1: Vec::new(),
                        extranonce2_size: 0,
                        share_difficulty: None,
                        version_mask: authorized_mask,
                    });
                }
            }

            ClientEvent::Subscribed {
                extranonce1,
                extranonce2_size,
            } => {
                info!(
                    pool = %self.config.url,
                    user = %self.config.username,
                    "Subscribed."
                );

                // Update or create protocol state
                // Preserve version_mask if already set by VersionRollingConfigured
                if let Some(state) = &mut self.state {
                    state.extranonce1 = extranonce1;
                    state.extranonce2_size = extranonce2_size;
                } else {
                    self.state = Some(ProtocolState {
                        extranonce1,
                        extranonce2_size,
                        share_difficulty: None,
                        version_mask: None,
                    });
                }
            }

            ClientEvent::NewJob(job) => {
                debug!(job_id = %job.job_id, clean_jobs = job.clean_jobs, "Received job from pool");

                let clean_jobs = job.clean_jobs;
                let template = self.job_to_template(job)?;
                let event = if clean_jobs {
                    SourceEvent::ReplaceJob(template)
                } else {
                    SourceEvent::UpdateJob(template)
                };

                self.event_tx.send(event).await?;
            }

            ClientEvent::DifficultyChanged(diff) => {
                let difficulty = Difficulty::from(diff);
                debug!(difficulty = %difficulty, "Pool difficulty changed");
                if let Some(state) = &mut self.state {
                    state.share_difficulty = Some(difficulty);
                }
            }

            ClientEvent::VersionMaskSet(mask) => {
                info!(mask = format!("{:#010x}", mask), "Version mask set");
                if let Some(state) = &mut self.state {
                    state.version_mask = Some(mask);
                }
            }

            ClientEvent::ShareAccepted { job_id, nonce } => {
                if !self.first_share_logged {
                    self.first_share_logged = true;
                    info!(
                        pool = %self.config.url,
                        user = %self.config.username,
                        nonce = format!("{:#x}", nonce),
                        job_id = %job_id,
                        "First share accepted."
                    );
                } else {
                    debug!(
                        pool = %self.config.url,
                        user = %self.config.username,
                        nonce = format!("{:#x}", nonce),
                        job_id = %job_id,
                        "Share accepted."
                    );
                }
            }

            ClientEvent::ShareRejected { job_id, reason } => {
                warn!(job_id = %job_id, reason = %reason, "Share rejected by pool");
            }

            ClientEvent::Disconnected => {
                warn!("Disconnected from pool");
                self.event_tx.send(SourceEvent::ClearJobs).await?;
            }

            ClientEvent::Error(err) => {
                warn!(error = %err, "Pool error");
            }
        }

        Ok(())
    }

    /// Convert Share to SubmitParams.
    fn share_to_submit_params(&self, share: Share) -> Result<crate::stratum_v1::SubmitParams> {
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No protocol state (not subscribed)"))?;

        // Extract extranonce2 from share (if present)
        let extranonce2 = share
            .extranonce2
            .map(Vec::from)
            .unwrap_or_else(|| vec![0; state.extranonce2_size]);

        // Extract version bits if version rolling was authorized
        // Always include version_bits parameter when pool authorized rolling,
        // even if the value is 0x00000000 (pool requires the field)
        let version_bits = state.version_mask.map(|mask| {
            let rolled = share.version.to_consensus() as u32;
            rolled & mask
        });

        Ok(crate::stratum_v1::SubmitParams {
            username: self.config.username.clone(),
            job_id: share.job_id,
            extranonce2,
            ntime: share.time,
            nonce: share.nonce,
            version_bits,
        })
    }

    /// Compute the suggested difficulty for the given hashrate.
    ///
    /// Returns `None` for zero hashrate (nothing to suggest yet).
    fn compute_suggested_difficulty(hashrate: HashRate) -> Option<u64> {
        if hashrate.is_zero() {
            return None;
        }
        let target = target_for_share_rate(SUGGESTED_SHARE_RATE, hashrate);
        let diff = Difficulty::from_target(target).as_u64().max(1);
        Some(diff)
    }

    /// Send `SuggestDifficulty` if the computed value changed materially
    /// (factor of 2) from the last suggestion.
    async fn maybe_suggest_difficulty(&mut self, client_command_tx: &mpsc::Sender<ClientCommand>) {
        let Some(new_diff) = Self::compute_suggested_difficulty(self.expected_hashrate) else {
            return;
        };

        let dominated = match self.last_suggested_difficulty {
            Some(prev) => {
                let ratio = new_diff as f64 / prev as f64;
                ratio >= MATERIAL_CHANGE_FACTOR || ratio <= 1.0 / MATERIAL_CHANGE_FACTOR
            }
            None => true,
        };

        if !dominated {
            return;
        }

        debug!(
            difficulty = new_diff,
            hashrate = %self.expected_hashrate,
            "Suggesting difficulty to pool"
        );
        self.last_suggested_difficulty = Some(new_diff);

        if let Err(e) = client_command_tx
            .send(ClientCommand::SuggestDifficulty(new_diff))
            .await
        {
            warn!(error = %e, "Failed to send suggest_difficulty to client");
        }
    }

    /// Run the source (main event loop).
    ///
    /// Defers pool connection until the scheduler provides a positive
    /// hashrate via `UpdateHashRate`, so `suggest_difficulty` always has a
    /// meaningful value at connect time. Once connected, re-suggests when
    /// hashrate changes materially (factor of 2).
    pub async fn run(mut self) -> Result<()> {
        info!(pool = %self.config.url, "Waiting for hashrate before connecting");

        // Phase 1: wait for a positive hashrate before connecting.
        // Drain commands; only UpdateHashRate matters here.
        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        SourceCommand::UpdateHashRate(rate) => {
                            self.expected_hashrate = rate;
                            if !rate.is_zero() {
                                break;
                            }
                        }
                        SourceCommand::SubmitShare(_) => {
                            // No connection yet, drop silently.
                        }
                    }
                }
                _ = self.shutdown.cancelled() => {
                    return Ok(());
                }
            }
        }

        // Phase 2: connect and run.
        debug!(
            pool = %self.config.url,
            hashrate = %self.expected_hashrate,
            "Hashrate available, connecting to pool"
        );

        // Create channels for client communication
        let (client_event_tx, mut client_event_rx) = mpsc::channel(100);
        let (client_command_tx, client_command_rx) = mpsc::channel(100);

        // Compute initial difficulty so the client can send it inline
        // during the handshake, before the first job arrives.
        let initial_difficulty = Self::compute_suggested_difficulty(self.expected_hashrate);
        self.last_suggested_difficulty = initial_difficulty;

        // Create the Stratum client with command channel
        let client = crate::stratum_v1::StratumV1Client::with_commands(
            self.config.clone(),
            client_event_tx,
            client_command_rx,
            self.shutdown.clone(),
            initial_difficulty,
        );

        // Spawn client task
        let client_handle = tokio::spawn(async move { client.run().await });

        // Main event loop
        loop {
            tokio::select! {
                // Events from Stratum client
                event_opt = client_event_rx.recv() => {
                    match event_opt {
                        Some(event) => {
                            if let Err(e) = self.handle_client_event(event).await {
                                warn!(error = %e, "Error handling client event");
                            }
                        }
                        None => {
                            warn!("Client event channel closed (client task exited)");
                            break;
                        }
                    }
                }

                // Commands from scheduler
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        SourceCommand::SubmitShare(share) => {
                            debug!(
                                pool = %self.name(),
                                job_id = %share.job_id,
                                nonce = format!("{:#x}", share.nonce),
                                "Submitting share"
                            );

                            // Convert share to Stratum format and send to client
                            match self.share_to_submit_params(share) {
                                Ok(submit_params) => {
                                    if let Err(e) = client_command_tx.send(
                                        ClientCommand::SubmitShare(submit_params)
                                    ).await {
                                        warn!(error = %e, "Failed to send share to client");
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to convert share");
                                }
                            }
                        }

                        SourceCommand::UpdateHashRate(rate) => {
                            self.expected_hashrate = rate;
                            self.maybe_suggest_difficulty(&client_command_tx).await;
                        }
                    }
                }

                // Shutdown
                _ = self.shutdown.cancelled() => {
                    break;
                }
            }
        }

        // Wait for client to finish and propagate any errors
        match client_handle.await? {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(error = %e, "Stratum client failed");
                Err(e.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asic::bm13xx::test_data::esp_miner_job::{
        POOL_SHARE_DIFFICULTY_INT, STRATUM_EXTRANONCE1, STRATUM_EXTRANONCE2_SIZE, VERSION_MASK,
        notify, submit,
    };
    use crate::asic::bm13xx::test_data::stratum_json;
    use crate::job_source::Extranonce2;
    use crate::stratum_v1::JobNotification;
    use bitcoin::block::Version;
    use serde_json::json;

    /// Helper to convert extranonce2 bytes to Extranonce2 type.
    fn extranonce2_from_bytes(bytes: &[u8]) -> Extranonce2 {
        let size = bytes.len() as u8;
        // Pad to 8 bytes for u64 conversion (little-endian)
        let mut padded = [0u8; 8];
        padded[..bytes.len()].copy_from_slice(bytes);
        let value = u64::from_le_bytes(padded);
        Extranonce2::new(value, size).unwrap()
    }

    /// Helper to create a StratumV1Source with protocol state for testing.
    fn source_with_state(
        extranonce1: Vec<u8>,
        extranonce2_size: usize,
        share_difficulty: Option<u64>,
        version_mask: Option<u32>,
    ) -> StratumV1Source {
        let (event_tx, _event_rx) = mpsc::channel(10);
        let (_command_tx, command_rx) = mpsc::channel(10);
        let shutdown = CancellationToken::new();

        let config = PoolConfig {
            url: "stratum+tcp://test:3333".to_string(),
            username: "testworker".to_string(),
            password: "x".to_string(),
            user_agent: "test".to_string(),
            ..Default::default()
        };

        let mut source = StratumV1Source::new(config, command_rx, event_tx, shutdown);

        source.state = Some(ProtocolState {
            extranonce1,
            extranonce2_size,
            share_difficulty: share_difficulty.map(Difficulty::from),
            version_mask,
        });

        source
    }

    /// Test job_to_template with real capture data from esp-miner.
    ///
    /// Uses the Bitaxe Gamma capture that produced an accepted share at
    /// difficulty 29588. Validates that Stratum JobNotification converts
    /// correctly to internal JobTemplate.
    #[test]
    fn test_job_to_template_with_capture_data() {
        // Create source with protocol state matching the capture
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1.clone(),
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            Some(VERSION_MASK),
        );

        // Parse the actual wire message
        let json: serde_json::Value = serde_json::from_str(stratum_json::MINING_NOTIFY)
            .expect("Failed to parse MINING_NOTIFY JSON");
        let params = json["params"].as_array().expect("params not an array");

        let job =
            JobNotification::from_stratum_params(params).expect("Failed to parse capture data");

        // Convert to JobTemplate
        let template = source.job_to_template(job).expect("job_to_template failed");

        // Validate job ID preserved
        assert_eq!(template.id, notify::JOB_ID_STRING);

        // Validate prev_blockhash matches wire capture
        assert_eq!(
            template.prev_blockhash,
            *notify::PREV_BLOCKHASH,
            "prev_blockhash mismatch"
        );

        // Validate version template has correct base and mask
        assert_eq!(
            template.version.base(),
            *notify::VERSION,
            "Base version mismatch"
        );

        // Version mask should allow GP bits (bits 13-28)
        let gp_mask = template.version.gp_bits_mask();
        assert_eq!(
            gp_mask.as_bytes(),
            &[0xff, 0xff],
            "GP bits mask should be full (0xffff) for mask 0x1fffe000"
        );

        // Validate bits (network difficulty)
        assert_eq!(template.bits, *notify::NBITS, "nbits mismatch");

        // Validate timestamp
        assert_eq!(template.time, *notify::NTIME, "ntime mismatch");

        // Validate merkle root template
        match &template.merkle_root {
            MerkleRootKind::Computed(mrt) => {
                assert_eq!(
                    mrt.coinbase1,
                    hex::decode(notify::COINBASE1).unwrap(),
                    "coinbase1 mismatch"
                );
                assert_eq!(mrt.extranonce1, extranonce1, "extranonce1 mismatch");
                assert_eq!(
                    mrt.coinbase2,
                    hex::decode(notify::COINBASE2).unwrap(),
                    "coinbase2 mismatch"
                );
                assert_eq!(
                    mrt.merkle_branches.len(),
                    12,
                    "Wrong number of merkle branches"
                );
            }
            MerkleRootKind::Fixed(_) => panic!("Expected Computed merkle root"),
        }

        // Validate share target was computed from difficulty
        // At difficulty 8192, the target should allow shares easier than network difficulty
        let share_difficulty_float = template.share_target.difficulty_float();
        assert!(
            (share_difficulty_float - POOL_SHARE_DIFFICULTY_INT as f64).abs() < 1.0,
            "Share target difficulty mismatch: got {}, expected {}",
            share_difficulty_float,
            POOL_SHARE_DIFFICULTY_INT
        );
    }

    /// Test job_to_template without version rolling support.
    ///
    /// When pool doesn't authorize version rolling, the version_mask should
    /// be None and the GP bits mask should be zero (no rolling allowed).
    #[test]
    fn test_job_to_template_without_version_rolling() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            None, // No version rolling
        );

        let params = json!([
            "jobid",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "aa",
            "bb",
            [],
            "20000000",
            "1d00ffff",
            "5a5a5a5a",
            false
        ]);

        let job = JobNotification::from_stratum_params(params.as_array().unwrap()).unwrap();
        let template = source.job_to_template(job).unwrap();

        // GP bits mask should be zero when no version rolling
        let gp_mask = template.version.gp_bits_mask();
        assert_eq!(
            gp_mask.as_bytes(),
            &[0x00, 0x00],
            "GP bits mask should be zero without version rolling"
        );
    }

    /// Test job_to_template uses default difficulty when not set.
    #[test]
    fn test_job_to_template_default_difficulty() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            None, // No difficulty set yet
            Some(VERSION_MASK),
        );

        let params = json!([
            "jobid",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "aa",
            "bb",
            [],
            "20000000",
            "1d00ffff",
            "5a5a5a5a",
            false
        ]);

        let job = JobNotification::from_stratum_params(params.as_array().unwrap()).unwrap();
        let template = source.job_to_template(job).unwrap();

        // Should default to difficulty 1 (Target::MAX)
        let share_difficulty_float = template.share_target.difficulty_float();
        assert!(
            share_difficulty_float < 2.0,
            "Default difficulty should be ~1, got {}",
            share_difficulty_float
        );
    }

    /// Test share_to_submit_params with real capture data.
    ///
    /// Converts the share found by the Bitaxe Gamma back to Stratum format
    /// and validates against the actual mining.submit from the capture.
    #[test]
    fn test_share_to_submit_params_with_capture_data() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            Some(VERSION_MASK),
        );

        // Build Share from capture data
        // The version in Share should be the FULL version (base | rolled bits)
        let full_version = Version::from_consensus(*submit::VERSION as i32 | 0x20000000);

        let share = Share {
            job_id: submit::JOB_ID_STRING.to_string(),
            nonce: *submit::NONCE,
            time: *submit::NTIME,
            version: full_version,
            extranonce2: Some(extranonce2_from_bytes(&*submit::EXTRANONCE2)),
        };

        // Convert to SubmitParams
        let params = source
            .share_to_submit_params(share)
            .expect("share_to_submit_params failed");

        // Validate username
        assert_eq!(params.username, "testworker");

        // Validate job ID
        assert_eq!(params.job_id, submit::JOB_ID_STRING);

        // Validate extranonce2
        assert_eq!(
            params.extranonce2,
            submit::EXTRANONCE2.to_vec(),
            "extranonce2 mismatch"
        );

        // Validate ntime
        assert_eq!(params.ntime, *submit::NTIME, "ntime mismatch");

        // Validate nonce
        assert_eq!(params.nonce, *submit::NONCE, "nonce mismatch");

        // Validate version_bits (should be rolled bits only, masked)
        // The capture shows version_bits = 0x00b44000
        assert_eq!(
            params.version_bits,
            Some(*submit::VERSION),
            "version_bits mismatch"
        );
    }

    /// Test share_to_submit_params without version rolling.
    ///
    /// When pool didn't authorize version rolling, version_bits should be None.
    #[test]
    fn test_share_to_submit_params_without_version_rolling() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            None, // No version rolling
        );

        let share = Share {
            job_id: "testjob".to_string(),
            nonce: 0x12345678,
            time: 0x65432100,
            version: Version::from_consensus(0x20000000),
            extranonce2: Some(extranonce2_from_bytes(&[0xde, 0xad, 0xbe, 0xef])),
        };

        let params = source.share_to_submit_params(share).unwrap();

        // version_bits should be None when no version rolling authorized
        assert_eq!(
            params.version_bits, None,
            "version_bits should be None without version rolling"
        );
    }

    /// Test share_to_submit_params uses default extranonce2 when not provided.
    #[test]
    fn test_share_to_submit_params_default_extranonce2() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            None,
        );

        let share = Share {
            job_id: "testjob".to_string(),
            nonce: 0x12345678,
            time: 0x65432100,
            version: Version::from_consensus(0x20000000),
            extranonce2: None, // Not provided
        };

        let params = source.share_to_submit_params(share).unwrap();

        // Should default to zeros with correct size
        assert_eq!(
            params.extranonce2,
            vec![0u8; STRATUM_EXTRANONCE2_SIZE],
            "Default extranonce2 should be zeros"
        );
    }

    /// Test SubmitParams serialization matches expected wire format.
    ///
    /// Validates the complete round-trip: Share → SubmitParams → JSON matches
    /// the actual mining.submit from the capture.
    #[test]
    fn test_submit_params_wire_format_matches_capture() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            Some(VERSION_MASK),
        );

        // Build Share from capture
        let full_version = Version::from_consensus(*submit::VERSION as i32 | 0x20000000);

        let share = Share {
            job_id: submit::JOB_ID_STRING.to_string(),
            nonce: *submit::NONCE,
            time: *submit::NTIME,
            version: full_version,
            extranonce2: Some(extranonce2_from_bytes(&*submit::EXTRANONCE2)),
        };

        // Convert to SubmitParams and then to JSON
        let params = source.share_to_submit_params(share).unwrap();
        let json = params.to_stratum_json();

        // Validate each field matches capture
        // params[0] = username (we use "testworker")
        // params[1] = job_id
        assert_eq!(json[1].as_str().unwrap(), submit::JOB_ID_STRING);

        // params[2] = extranonce2 (hex)
        assert_eq!(json[2].as_str().unwrap(), submit::EXTRANONCE2_STRING);

        // params[3] = ntime (hex)
        assert_eq!(json[3].as_str().unwrap(), submit::NTIME_STRING);

        // params[4] = nonce (hex)
        assert_eq!(json[4].as_str().unwrap(), submit::NONCE_STRING);

        // params[5] = version_bits (hex)
        assert_eq!(json[5].as_str().unwrap(), submit::VERSION_STRING);
    }

    /// Test merkle root computation through job template.
    ///
    /// Validates that the JobTemplate can compute the correct merkle root
    /// using the extranonce2 from the capture.
    #[test]
    fn test_job_template_merkle_root_computation() {
        let extranonce1 = hex::decode(STRATUM_EXTRANONCE1).unwrap();
        let source = source_with_state(
            extranonce1,
            STRATUM_EXTRANONCE2_SIZE,
            Some(POOL_SHARE_DIFFICULTY_INT),
            Some(VERSION_MASK),
        );

        // Parse the actual wire message
        let json: serde_json::Value = serde_json::from_str(stratum_json::MINING_NOTIFY)
            .expect("Failed to parse MINING_NOTIFY JSON");
        let params = json["params"].as_array().expect("params not an array");

        let job = JobNotification::from_stratum_params(params).unwrap();
        let template = source.job_to_template(job).unwrap();

        // Compute merkle root using the extranonce2 from the capture
        let en2 = extranonce2_from_bytes(&*submit::EXTRANONCE2);
        let merkle_root = template
            .compute_merkle_root(&en2)
            .expect("Merkle root computation failed");

        // This should match the merkle root in the wire capture
        assert_eq!(
            merkle_root,
            *notify::MERKLE_ROOT,
            "Computed merkle root doesn't match capture"
        );
    }

    #[test]
    fn test_compute_suggested_difficulty_zero_hashrate() {
        assert_eq!(
            StratumV1Source::compute_suggested_difficulty(HashRate::default()),
            None,
        );
    }

    #[test]
    fn test_compute_suggested_difficulty_bitaxe_gamma() {
        // ~500 GH/s at 20 shares/min (3-sec interval) should yield ~349
        let diff = StratumV1Source::compute_suggested_difficulty(HashRate::from_gigahashes(500.0))
            .unwrap();
        assert!(
            (300..400).contains(&diff),
            "Bitaxe Gamma difficulty {diff} not in expected range 300..400"
        );
    }

    #[test]
    fn test_compute_suggested_difficulty_always_at_least_one() {
        // Even at very low hashrate, difficulty should be at least 1
        let diff =
            StratumV1Source::compute_suggested_difficulty(HashRate::from_megahashes(1.0)).unwrap();
        assert!(diff >= 1);
    }

    #[tokio::test]
    async fn test_maybe_suggest_difficulty_first_call_always_sends() {
        let (event_tx, _event_rx) = mpsc::channel(10);
        let (_command_tx, command_rx) = mpsc::channel(10);
        let shutdown = CancellationToken::new();
        let config = PoolConfig {
            url: "stratum+tcp://test:3333".to_string(),
            ..Default::default()
        };

        let mut source = StratumV1Source::new(config, command_rx, event_tx, shutdown);
        source.expected_hashrate = HashRate::from_terahashes(1.0);

        let (client_tx, mut client_rx) = mpsc::channel(10);
        source.maybe_suggest_difficulty(&client_tx).await;

        let cmd = client_rx.try_recv().expect("should have sent command");
        match cmd {
            ClientCommand::SuggestDifficulty(d) => {
                assert!(d > 0, "difficulty should be positive");
            }
            other => panic!("expected SuggestDifficulty, got {other:?}"),
        }
        assert!(source.last_suggested_difficulty.is_some());
    }

    #[tokio::test]
    async fn test_maybe_suggest_difficulty_suppresses_small_changes() {
        let (event_tx, _event_rx) = mpsc::channel(10);
        let (_command_tx, command_rx) = mpsc::channel(10);
        let shutdown = CancellationToken::new();
        let config = PoolConfig {
            url: "stratum+tcp://test:3333".to_string(),
            ..Default::default()
        };

        let mut source = StratumV1Source::new(config, command_rx, event_tx, shutdown);
        source.expected_hashrate = HashRate::from_terahashes(1.0);

        let (client_tx, mut client_rx) = mpsc::channel(10);

        // First call sends
        source.maybe_suggest_difficulty(&client_tx).await;
        let _ = client_rx.try_recv().unwrap();

        // Same hashrate again -- no material change, should not send
        source.maybe_suggest_difficulty(&client_tx).await;
        assert!(
            client_rx.try_recv().is_err(),
            "should not re-suggest for same hashrate"
        );
    }

    #[tokio::test]
    async fn test_maybe_suggest_difficulty_sends_on_material_change() {
        let (event_tx, _event_rx) = mpsc::channel(10);
        let (_command_tx, command_rx) = mpsc::channel(10);
        let shutdown = CancellationToken::new();
        let config = PoolConfig {
            url: "stratum+tcp://test:3333".to_string(),
            ..Default::default()
        };

        let mut source = StratumV1Source::new(config, command_rx, event_tx, shutdown);
        source.expected_hashrate = HashRate::from_terahashes(1.0);

        let (client_tx, mut client_rx) = mpsc::channel(10);

        // First call
        source.maybe_suggest_difficulty(&client_tx).await;
        let _ = client_rx.try_recv().unwrap();

        // Double hashrate -- 2x change, should re-suggest
        source.expected_hashrate = HashRate::from_terahashes(2.5);
        source.maybe_suggest_difficulty(&client_tx).await;
        assert!(
            client_rx.try_recv().is_ok(),
            "should re-suggest after 2x hashrate change"
        );
    }

    #[tokio::test]
    async fn test_maybe_suggest_difficulty_skips_zero_hashrate() {
        let (event_tx, _event_rx) = mpsc::channel(10);
        let (_command_tx, command_rx) = mpsc::channel(10);
        let shutdown = CancellationToken::new();
        let config = PoolConfig {
            url: "stratum+tcp://test:3333".to_string(),
            ..Default::default()
        };

        let mut source = StratumV1Source::new(config, command_rx, event_tx, shutdown);
        // hashrate is zero by default

        let (client_tx, mut client_rx) = mpsc::channel(10);
        source.maybe_suggest_difficulty(&client_tx).await;
        assert!(
            client_rx.try_recv().is_err(),
            "should not suggest with zero hashrate"
        );
    }

    #[test]
    fn backoff_doubles_each_step() {
        let mut backoff = ExponentialBackoff::new(Duration::from_secs(1), Duration::from_secs(60));

        let d1 = backoff.next_delay();
        let d2 = backoff.next_delay();
        let d3 = backoff.next_delay();

        // Nominal sequence is 1s, 2s, 4s. With jitter in [0.5, 1.0],
        // each delay is at least half the nominal value.
        assert!(d1 >= Duration::from_millis(500), "d1={d1:?}");
        assert!(d1 < Duration::from_secs(1), "d1={d1:?}");

        assert!(d2 >= Duration::from_secs(1), "d2={d2:?}");
        assert!(d2 < Duration::from_secs(2), "d2={d2:?}");

        assert!(d3 >= Duration::from_secs(2), "d3={d3:?}");
        assert!(d3 < Duration::from_secs(4), "d3={d3:?}");
    }

    #[test]
    fn backoff_caps_at_max() {
        let mut backoff = ExponentialBackoff::new(Duration::from_secs(32), Duration::from_secs(60));

        let _d1 = backoff.next_delay(); // consumes 32s nominal
        let d2 = backoff.next_delay(); // nominal capped at 60s

        assert!(d2 >= Duration::from_secs(30), "d2={d2:?}");
        assert!(d2 < Duration::from_secs(60), "d2={d2:?}");

        // Further calls stay at max
        let d3 = backoff.next_delay();
        assert!(d3 >= Duration::from_secs(30), "d3={d3:?}");
        assert!(d3 < Duration::from_secs(60), "d3={d3:?}");
    }

    #[test]
    fn backoff_reset_restores_initial() {
        let mut backoff = ExponentialBackoff::new(Duration::from_secs(1), Duration::from_secs(60));

        // Advance past initial
        let _ = backoff.next_delay();
        let _ = backoff.next_delay();
        let _ = backoff.next_delay();

        backoff.reset();

        let d = backoff.next_delay();
        assert!(d >= Duration::from_millis(500), "d={d:?}");
        assert!(d < Duration::from_secs(1), "d={d:?}");
    }
}
