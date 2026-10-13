//! Reproducible ARQ stress tests. Long runs are explicit to keep ordinary CI bounded.
use super::*;
use crate::packet::{
    Ack, Nack, read_packet_ack, read_packet_nack, write_packet_ack, write_packet_nack,
};
use rand::{Rng, SeedableRng, rngs::StdRng, seq::SliceRandom};
use std::collections::BTreeMap;

fn peer() -> SocketAddr {
    "127.0.0.1:19342".parse().unwrap()
}

fn payload(id: u64) -> Vec<u8> {
    let mut data = vec![0xfe; 33];
    data[1..9].copy_from_slice(&id.to_le_bytes());
    for (offset, byte) in data[9..].iter_mut().enumerate() {
        *byte = id.wrapping_mul(31).wrapping_add(offset as u64) as u8;
    }
    data
}

fn deliver(receive: &mut RecvQ, frame: &FrameSetPacket, expected: &mut u64) {
    // Exercise the real 24-bit wire codec, not only in-memory frame indexes.
    for decoded in FrameVec::new(&frame.serialize().unwrap()).unwrap().frames {
        receive.insert(decoded).unwrap();
    }
    for message in receive.flush(&peer()) {
        assert_eq!(
            message.data.as_ref(),
            payload(*expected),
            "duplicate, missing, reordered or corrupt message at {expected}"
        );
        *expected += 1;
    }
}

fn ack_wire(ranges: Vec<(u32, u32)>) -> Vec<u8> {
    write_packet_ack(&Ack {
        record_count: ranges.len() as u16,
        sequences: ranges,
    })
    .unwrap()
}

fn apply_ack(send: &mut SendQ, wire: &[u8], tick: i64) {
    send.ack_ranges(&read_packet_ack(wire).unwrap().sequences, tick);
}

fn wrap_cycle(count: u64, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut send = SendQ::new(1400);
    let mut receive = RecvQ::new();
    let mut next = 0;
    let mut delivered = 0;
    let mut tick = 0;
    let mut delayed: VecDeque<Vec<u8>> = VecDeque::new();
    let mut dropped = 0_u64;
    let mut retries = 0_u64;
    let mut duplicates = 0_u64;
    let mut wraps = 0_u64;
    let mut previous_sequence = 0;
    while next < count {
        let end = (next + 64).min(count);
        for id in next..end {
            send.insert(Reliability::ReliableOrdered, &payload(id))
                .unwrap();
        }
        let mut frames = send.flush(tick, &peer()).frames;
        for frame in &frames {
            wraps += u64::from(frame.sequence_number < previous_sequence);
            previous_sequence = frame.sequence_number;
        }
        if delayed.len()
            > if count > sequence::MASK as u64 {
                4096
            } else {
                64
            }
        {
            let old = delayed.pop_front().unwrap();
            let outstanding = send.get_sent_queue_size();
            apply_ack(&mut send, &old, tick + 1);
            apply_ack(&mut send, &old, tick + 2);
            assert_eq!(
                send.get_sent_queue_size(),
                outstanding,
                "old ACK released current flight"
            );
        }
        frames.shuffle(&mut rng);
        let damage = next / 64 % 32 == 0;
        for frame in &frames {
            if damage && rng.gen_ratio(1, 3) {
                dropped += 1;
                send.nack(frame.sequence_number, tick + 1);
                continue;
            }
            deliver(&mut receive, frame, &mut delivered);
            if rng.gen_ratio(1, 8) {
                deliver(&mut receive, frame, &mut delivered);
                duplicates += 1;
            }
        }
        let mut retransmissions = send.flush(tick + 2, &peer()).frames;
        for frame in &retransmissions {
            wraps += u64::from(frame.sequence_number < previous_sequence);
            // Count wraps from transmission order separately from arrival order.
            previous_sequence = frame.sequence_number;
        }
        retransmissions.shuffle(&mut rng);
        for frame in &retransmissions {
            deliver(&mut receive, frame, &mut delivered);
            retries += 1;
        }
        assert_eq!(delivered, end);
        let ack = ack_wire(receive.get_ack());
        // Make a real timeout happen while its ACK is withheld, then ACK an older
        // transmission after the timeout retry has arrived in random order.
        let deadline = tick + send.get_rto() + 1;
        if next / 64 % 8 == 0 {
            let mut timed_out = send.flush(deadline, &peer()).frames;
            timed_out.shuffle(&mut rng);
            assert!(!timed_out.is_empty());
            for frame in &timed_out {
                deliver(&mut receive, frame, &mut delivered);
                retries += 1;
            }
            let _ = receive.get_ack();
        }
        apply_ack(&mut send, &ack, deadline + 10);
        assert!(send.is_empty());
        assert_eq!(send.buffered_bytes, 0);
        // Lost transmission IDs can remain holes even when a retry delivered
        // every reliable message. Feedback for retired IDs must be harmless.
        send.nack_ranges(&receive.get_nack(), deadline + 10);
        assert!(send.is_empty());
        // Thousands of old ACK packets overlap later windows, with duplicate
        // ACKs replayed after randomized, bounded delays. They may not release
        // unrelated frames or corrupt byte accounting.
        delayed.push_back(ack);
        next = end;
        tick = deadline + 20;
        if next % (1 << 22) == 0 {
            eprintln!("full-wrap progress: {next}/{count} messages");
        }
    }
    // A final probe proves delivery continues after the full epoch instead of
    // only having drained a pre-wrap stream.
    send.insert(Reliability::ReliableOrdered, &payload(count))
        .unwrap();
    let frames = send.flush(tick, &peer());
    for old in delayed {
        apply_ack(&mut send, &old, tick + 1);
    }
    assert_eq!(
        send.get_sent_queue_size(),
        1,
        "old ACK released the new probe"
    );
    for frame in frames {
        deliver(&mut receive, &frame, &mut delivered);
    }
    apply_ack(&mut send, &ack_wire(receive.get_ack()), tick + 2);
    assert_eq!(delivered, count + 1);
    assert!(send.is_empty());
    assert_eq!(send.buffered_bytes, 0);
    assert_eq!(receive.get_ordered_packet(), 0);
    assert_eq!(receive.get_fragment_queue_size(), 0);
    assert_eq!(
        send.reliable_frame_index,
        ((count + 1) as u32) & sequence::MASK
    );
    assert_eq!(
        send.ordered_frame_indexes.get(0),
        ((count + 1) as u32) & sequence::MASK
    );
    if count > sequence::MASK as u64 {
        assert!(wraps > 0);
    }
    eprintln!(
        "full-wrap complete: seed={seed}, delivered={delivered}, intentional_drops={dropped}, retries={retries}, injected_duplicates={duplicates}, virtual_ms={tick}"
    );
}

#[test]
fn shuffled_wire_frames_and_delayed_duplicate_acks_preserve_delivery() {
    wrap_cycle(8192, 0x24_00_01);
}

#[test]
#[ignore = "explicit long stress run: traverses all 16,777,216 serial values"]
fn complete_24_bit_epoch_with_reordering_loss_and_delayed_acks() {
    wrap_cycle((1_u64 << 24) + 64, 0x24_f00d);
}

#[derive(Default)]
struct DamageStats {
    data_drops: u64,
    ack_drops: u64,
    nack_drops: u64,
    duplicates: u64,
    transmissions: u64,
}

enum Event {
    Data(Vec<u8>),
    Ack(Vec<u8>),
    Nack(Vec<u8>),
}

fn virtual_damage(duration_ms: i64, seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut send = SendQ::new(1400);
    let mut receive = RecvQ::new();
    // The all-feature run uses the four seeds to cover send limits, recovery
    // options, both policies and the unchanged default queue respectively.
    #[cfg(feature = "send-policy")]
    if seed & 1 != 0 {
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
    }
    #[cfg(feature = "recovery-policy")]
    if seed & 2 != 0 {
        send.set_recovery_options(
            crate::RecoveryOptions {
                deadline_driven: true,
                tail_probe_min_delay: Some(std::time::Duration::from_millis(10)),
                reset_backoff_on_progress: true,
            },
            0,
        );
    }
    // Cross a wrap early while actual retries, stale feedback and queued data coexist.
    let start = sequence::MASK - 127;
    send.sequence_number = start;
    send.ack_sequence_number = sequence::MASK - 128;
    send.reliable_frame_index = start;
    send.ordered_frame_indexes.insert(0, start);
    receive.reliable_window.next = start;
    receive.last_ordered_indexes.insert(0, start);
    receive.sequence_number_ackset.next_expected = start;
    let mut pending: BTreeMap<i64, Vec<Event>> = BTreeMap::new();
    let mut stats = DamageStats::default();
    let mut next = 0;
    let mut delivered = 0;
    let mut peak_pending = 0;
    let mut last_tick = 0;
    for tick in (0..duration_ms + 180_000).step_by(50) {
        last_tick = tick;
        let injecting = tick < duration_ms;
        let outage = injecting && tick % 600_000 >= 570_000;
        if injecting && tick % 500 == 0 {
            for _ in 0..4 {
                if send.has_capacity(Reliability::ReliableOrdered, 33).unwrap() {
                    send.insert(Reliability::ReliableOrdered, &payload(next))
                        .unwrap();
                    next += 1;
                }
            }
        }
        while pending
            .first_key_value()
            .is_some_and(|(due, _)| *due <= tick)
        {
            let (_, mut events) = pending.pop_first().unwrap();
            events.shuffle(&mut rng);
            for event in events {
                match event {
                    Event::Data(wire) => {
                        for frame in FrameVec::new(&wire).unwrap().frames {
                            receive.insert(frame).unwrap();
                        }
                        for frame in receive.flush(&peer()) {
                            assert_eq!(
                                frame.data.as_ref(),
                                payload(delivered),
                                "seed={seed}, tick={tick}, expected={delivered}"
                            );
                            delivered += 1;
                        }
                    }
                    Event::Ack(wire) => apply_ack(&mut send, &wire, tick),
                    Event::Nack(wire) => {
                        send.nack_ranges(&read_packet_nack(&wire).unwrap().sequences, tick)
                    }
                }
            }
        }
        for frame in send.flush(tick, &peer()) {
            stats.transmissions += 1;
            if outage || (injecting && rng.gen_ratio(45, 100)) {
                stats.data_drops += 1;
                continue;
            }
            let delay = rng.gen_range(1..=4000);
            let wire = frame.serialize().unwrap();
            if injecting && rng.gen_ratio(1, 4) {
                stats.duplicates += 1;
                pending
                    .entry(tick + delay + rng.gen_range(1..=20_000))
                    .or_default()
                    .push(Event::Data(wire.clone()));
            }
            pending
                .entry(tick + delay)
                .or_default()
                .push(Event::Data(wire));
        }
        let acks = receive.get_ack();
        if !acks.is_empty() {
            if outage || (injecting && rng.gen_ratio(60, 100)) {
                stats.ack_drops += 1;
            } else {
                let wire = ack_wire(acks);
                let delay = rng.gen_range(500..=20_000);
                pending
                    .entry(tick + delay)
                    .or_default()
                    .push(Event::Ack(wire.clone()));
                if injecting && rng.gen_ratio(1, 2) {
                    pending
                        .entry(tick + delay + rng.gen_range(1..=40_000))
                        .or_default()
                        .push(Event::Ack(wire));
                }
            }
        }
        let nacks = receive.get_nack();
        if !nacks.is_empty() {
            if outage || (injecting && rng.gen_ratio(60, 100)) {
                stats.nack_drops += 1;
            } else {
                pending
                    .entry(tick + rng.gen_range(1..=4000))
                    .or_default()
                    .push(Event::Nack(
                        write_packet_nack(&Nack {
                            record_count: nacks.len() as u16,
                            sequences: nacks,
                        })
                        .unwrap(),
                    ));
            }
        }
        peak_pending = peak_pending.max(pending.values().map(Vec::len).sum::<usize>());
        assert!(
            peak_pending < 100_000,
            "simulation event backlog exceeded its bound"
        );
        if !injecting && pending.is_empty() && send.is_empty() {
            break;
        }
    }
    assert_eq!(delivered, next, "seed={seed}: omitted delivery");
    assert!(
        next >= (duration_ms / 500) as u64,
        "insufficient sustained offered load"
    );
    assert!(
        pending.is_empty(),
        "seed={seed}: network events did not drain"
    );
    assert!(
        send.is_empty(),
        "seed={seed}: unacknowledged messages remain"
    );
    assert_eq!(send.buffered_bytes, 0);
    assert_eq!(receive.get_ordered_packet(), 0);
    assert_eq!(receive.get_fragment_queue_size(), 0);
    assert!(stats.data_drops > 0 && stats.ack_drops > 0 && stats.nack_drops > 0);
    eprintln!(
        "virtual-damage complete: seed={seed}, virtual_ms={last_tick}, delivered={delivered}, transmissions={}, data_drops={}, ack_drops={}, nack_drops={}, duplicates={}, peak_events={peak_pending}",
        stats.transmissions, stats.data_drops, stats.ack_drops, stats.nack_drops, stats.duplicates
    );
}

#[test]
fn random_damage_and_delayed_feedback_cross_a_sequence_boundary() {
    virtual_damage(120_000, 0x5eed_01);
}

#[test]
#[ignore = "explicit long stress run: four seeded 24-hour virtual networks"]
fn sustained_24_hour_damage_with_delayed_acks_and_random_reordering() {
    for seed in [0x5eed_01, 0x5eed_02, 0x5eed_03, 0x5eed_04] {
        virtual_damage(24 * 60 * 60 * 1000, seed);
    }
}
