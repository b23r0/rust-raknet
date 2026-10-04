use std::time::Duration;

/// Optional latency-oriented recovery settings for one connection.
/// These settings do not change RakNet packets or delivery guarantees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryOptions {
    /// Wake maintenance at the earliest reliable retry deadline.
    /// Disabled by default to preserve the original periodic recovery cadence.
    /// Enabling tail probes also enables deadline-driven maintenance.
    pub deadline_driven: bool,
    /// Minimum delay before one early retransmission without ACK progress.
    /// `None` disables probing. Valid RTT feedback is required before probing. Only one outstanding
    /// datagram that has not been retransmitted and has no queued messages is eligible. Packed frames are replayed together.
    /// A probe preserves the original RTO deadline and normal timeout backoff.
    /// The delay is also bounded below by 1.5 times the smoothed RTT.
    /// Zero and submillisecond minimums use a one-millisecond floor.
    pub tail_probe_min_delay: Option<Duration>,
    /// Reduce pending frames' timeout backoff on new, unambiguous RTT feedback.
    /// Retransmitted frames remain excluded from RTT sampling.
    pub reset_backoff_on_progress: bool,
}
