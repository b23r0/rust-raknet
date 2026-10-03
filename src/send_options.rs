//! Connection-local send limits. These do not change the RakNet wire protocol.

const MAX_BYTES: usize = 64 * 1024 * 1024;

/// An explicit policy for outstanding reliable frames and two pending queues.
///
/// Apply with `RaknetSocket::set_send_options`. Unconfigured sockets retain
/// their original shared queue; `Default` only creates a configuration preset.
///
/// Limits account for payload and estimated frame metadata, not process RSS.
/// Unreliable traffic shares the network with reliable traffic; these limits
/// provide queue isolation and scheduling fairness, not bandwidth guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendOptions {
    pub(crate) in_flight_frames: usize,
    pub(crate) in_flight_bytes: usize,
    pub(crate) reliable_queue_bytes: usize,
    pub(crate) unreliable_queue_bytes: usize,
    pub(crate) flush_bytes: usize,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            in_flight_frames: 64,
            in_flight_bytes: MAX_BYTES,
            reliable_queue_bytes: 256 * 1024,
            unreliable_queue_bytes: 64 * 1024,
            flush_bytes: 256 * 1024,
        }
    }
}

/// Invalid connection-local send limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendOptionsError(&'static str);

impl std::fmt::Display for SendOptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for SendOptionsError {}

impl SendOptions {
    /// Set reliable frame and encoded-frame byte limits, including fragments.
    /// A frame limit is not a datagram limit: packed frames count separately.
    pub fn with_in_flight_limits(
        mut self,
        frames: usize,
        bytes: usize,
    ) -> Result<Self, SendOptionsError> {
        if !(1..=4096).contains(&frames) || !(2048..=MAX_BYTES).contains(&bytes) {
            return Err(SendOptionsError(
                "flight limits require 1..=4096 frames and 2048..=67108864 bytes",
            ));
        }
        self.in_flight_frames = frames;
        self.in_flight_bytes = bytes;
        Ok(self)
    }

    /// Reserve independent pending budgets. A larger reliable send, including a batch,
    /// can exceed its soft budget, but must leave the unreliable reserve free.
    pub fn with_queue_budgets(
        mut self,
        reliable: usize,
        unreliable: usize,
    ) -> Result<Self, SendOptionsError> {
        if reliable < 2048
            || unreliable < 2048
            || reliable
                .checked_add(unreliable)
                .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(SendOptionsError(
                "queue budgets must each be at least 2048 bytes and sum to at most 67108864 bytes",
            ));
        }
        self.reliable_queue_bytes = reliable;
        self.unreliable_queue_bytes = unreliable;
        Ok(self)
    }

    /// Bound a flush burst in encoded frame bytes. There is no timer to collect
    /// messages. Pending work beyond this bound uses existing maintenance.
    pub fn with_flush_budget(mut self, bytes: usize) -> Result<Self, SendOptionsError> {
        if !(2048..=MAX_BYTES).contains(&bytes) {
            return Err(SendOptionsError(
                "flush budget must be between 2048 and 67108864 bytes",
            ));
        }
        self.flush_bytes = bytes;
        Ok(self)
    }

    /// Maximum outstanding reliable frame count.
    pub fn in_flight_frames(self) -> usize {
        self.in_flight_frames
    }
    /// Maximum outstanding encoded reliable frame bytes.
    pub fn in_flight_bytes(self) -> usize {
        self.in_flight_bytes
    }
    /// Soft reliable queue budget, including outstanding reliable data.
    pub fn reliable_queue_bytes(self) -> usize {
        self.reliable_queue_bytes
    }
    /// Unreliable queue reserve, unavailable to newly admitted reliable data.
    pub fn unreliable_queue_bytes(self) -> usize {
        self.unreliable_queue_bytes
    }
    /// Maximum encoded frame bytes returned by a scheduled flush.
    pub fn flush_bytes(self) -> usize {
        self.flush_bytes
    }
}
