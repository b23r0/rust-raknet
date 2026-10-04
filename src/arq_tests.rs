use super::*;

fn peer() -> SocketAddr {
    "127.0.0.1:19132".parse().unwrap()
}

#[test]
fn retransmitted_reliable_frame_is_acknowledged_but_delivered_once() {
    let mut recv = RecvQ::new();
    let mut frame = FrameSetPacket::new(Reliability::Reliable, vec![0xfe, 7]);
    recv.insert(frame.clone()).unwrap();
    assert_eq!(recv.flush(&peer()).len(), 1);
    frame.sequence_number = 4;
    recv.insert(frame).unwrap();
    assert!(recv.flush(&peer()).is_empty());
    assert!(recv.get_ack().contains(&(4, 4)));
}

#[test]
fn two_reliable_frames_in_one_datagram_are_not_confused_with_duplicates() {
    let mut recv = RecvQ::new();
    for index in 0..2 {
        let mut frame = FrameSetPacket::new(Reliability::Reliable, vec![0xfe, index]);
        frame.reliable_frame_index = u32::from(index);
        recv.insert(frame).unwrap();
        assert_eq!(recv.flush(&peer())[0].data.as_ref(), &[0xfe, index]);
    }
    assert_eq!(recv.get_ack(), vec![(0, 0)]);
}

#[test]
fn reliable_ordered_delivery_and_acks_cross_the_wire_wrap() {
    let mut send = SendQ::new(1400);
    send.sequence_number = sequence::MASK;
    send.reliable_frame_index = sequence::MASK;
    send.ack_sequence_number = sequence::MASK - 1;
    send.ordered_frame_indexes.insert(0, sequence::MASK);
    let mut recv = RecvQ::new();
    recv.reliable_window.next = sequence::MASK;
    recv.last_ordered_indexes.insert(0, sequence::MASK);
    recv.sequence_number_ackset.next_expected = sequence::MASK;
    for value in 0..2 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, value])
            .unwrap();
    }
    let frames = send.flush(0, &peer());
    assert_eq!(
        frames.iter().map(|f| f.sequence_number).collect::<Vec<_>>(),
        vec![sequence::MASK, 0]
    );
    // Deliver in reverse order after an actual wire serialization round trip.
    for frame in frames.iter().rev() {
        let wire = frame.serialize().unwrap();
        recv.insert(FrameVec::new(&wire).unwrap().frames.remove(0))
            .unwrap();
    }
    let delivered = recv.flush(&peer());
    assert_eq!(
        delivered.iter().map(|f| f.data[1]).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!(recv.get_nack().is_empty());
    send.ack_ranges(&recv.get_ack(), 1);
    assert!(send.is_empty());
    assert_eq!(send.buffered_bytes, 0);
}

#[test]
fn compound_id_wraps_without_debug_overflow() {
    let mut send = SendQ::new(1400);
    send.compound_id = u16::MAX;
    for _ in 0..2 {
        send.insert(Reliability::ReliableOrdered, &[0xfe; 2000])
            .unwrap();
    }
    let frames = send.flush(0, &peer());
    assert_eq!(frames[0].compound_id, u16::MAX);
    assert_eq!(frames[2].compound_id, 0);
}

#[test]
fn ack_and_nack_ranges_visit_only_outstanding_frames() {
    let mut send = SendQ::new(1400);
    for _ in 0..64 {
        send.insert(Reliability::Reliable, &[0xfe]).unwrap();
    }
    send.flush(0, &peer());
    send.ack(sequence::MASK - 1, 1);
    assert_eq!(send.get_sent_queue_size(), 64);
    assert!(send.sent_packet.iter().all(|item| item.1));
    send.nack_ranges(&[(0, sequence::MASK)], 1);
    assert_eq!(send.flush(1, &peer()).len(), 64);
    // Delayed acknowledgements of the original datagrams still release frames.
    send.ack_ranges(&[(0, 63)], 2);
    assert!(send.is_empty());
    assert_eq!(send.buffered_bytes, 0);
}

#[test]
fn queue_budget_counts_frame_overhead_and_is_released_by_ack() {
    let mut send = SendQ::new(1400);
    assert!(
        send.required_bytes(Reliability::ReliableOrdered, SendQ::MAX_BUFFERED_BYTES)
            .is_err()
    );
    send.insert(Reliability::ReliableOrdered, &[0xfe; 3000])
        .unwrap();
    assert!(send.buffered_bytes > 3000);
    let frames = send.flush(0, &peer());
    for frame in frames {
        send.ack(frame.sequence_number, 1);
    }
    assert_eq!(send.buffered_bytes, 0);
}

#[test]
fn full_reliable_window_still_allows_unreliable_messages() {
    let mut send = SendQ::new(1400);
    for _ in 0..100 {
        send.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
    }
    assert_eq!(send.flush(0, &peer()).len(), 64);
    send.insert(Reliability::Unreliable, &[0xfe, 42]).unwrap();
    let frames = send.flush(1, &peer());
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].data.as_ref(), &[0xfe, 42]);
    assert_eq!(send.get_reliable_queue_size(), 36);
}

#[test]
fn reordered_hole_is_not_nacked_after_it_arrives() {
    let mut ack = ACKSet::default();
    ack.insert(2);
    assert_eq!(ack.nack, vec![(0, 1)]);
    ack.insert(0);
    ack.insert(1);
    assert!(ack.get_nack().is_empty());
}

#[test]
fn sequenced_frames_cross_the_24_bit_wrap() {
    for reliability in [
        Reliability::UnreliableSequenced,
        Reliability::ReliableSequenced,
    ] {
        let mut recv = RecvQ::new();
        recv.sequenced_frame_indexes.insert(0, sequence::MASK);
        for (id, index) in [sequence::MASK, 0].into_iter().enumerate() {
            let mut frame = FrameSetPacket::new(reliability, vec![0xfe]);
            frame.sequence_number = id as u32;
            frame.reliable_frame_index = id as u32;
            frame.sequenced_frame_index = index;
            recv.insert(frame).unwrap();
            assert_eq!(recv.flush(&peer()).len(), 1);
        }
    }
}

#[test]
fn fragment_limit_rejects_a_peer_before_allocating_its_declared_count() {
    let mut recv = RecvQ::new();
    let mut frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe]);
    frame.flags |= 16;
    frame.compound_size = u32::MAX;
    assert!(recv.insert(frame).is_err());
    assert_eq!(recv.get_fragment_queue_size(), 0);
}

#[test]
fn backpressure_bounds_bursts_and_admits_a_single_large_message() {
    let mut send = SendQ::new(1400);
    let packet = [0xfe; 800];
    while send
        .has_capacity(Reliability::ReliableOrdered, packet.len())
        .unwrap()
    {
        send.insert(Reliability::ReliableOrdered, &packet).unwrap();
    }
    assert!(send.buffered_bytes <= SendQ::SEND_HIGH_WATER);
    assert!(
        !send
            .has_capacity(Reliability::ReliableOrdered, 2 * SendQ::SEND_HIGH_WATER)
            .unwrap()
    );
    let empty = SendQ::new(1400);
    assert!(
        empty
            .has_capacity(Reliability::ReliableOrdered, 2 * SendQ::SEND_HIGH_WATER)
            .unwrap()
    );
}

#[test]
fn retransmission_shares_payload_and_does_not_sample_ambiguous_rtt() {
    let mut send = SendQ::new(1400);
    send.insert(Reliability::Reliable, &[0xfe; 100]).unwrap();
    let first = send.flush(0, &peer()).remove(0);
    send.nack(first.sequence_number, 1);
    let repeated = send.flush(1, &peer()).remove(0);
    assert_eq!(first.data.as_ptr(), repeated.data.as_ptr());
    send.ack(first.sequence_number, 10_000);
    assert_eq!(send.get_rto(), SendQ::DEFAULT_TIMEOUT_MILLS);
}

#[test]
fn ack_history_allocates_only_after_retransmission_and_accepts_old_ids() {
    let peer = "127.0.0.1:0".parse().unwrap();
    for by_nack in [false, true] {
        let mut queue = SendQ::new(1400);
        queue.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        queue.flush(0, &peer);
        assert!(queue.sent_packet[0].4.is_empty());
        if by_nack {
            queue.nack(0, 1);
        }
        let resent = queue.flush(100, &peer);
        assert_eq!(resent.len(), 1);
        assert_eq!(queue.sent_packet[0].4, vec![0]);
        queue.ack(0, 101);
        assert!(queue.is_empty());
        assert_eq!(queue.buffered_bytes, 0);
    }
}

#[test]
fn batch_ack_releases_only_matching_frames_and_retries_only_real_gaps() {
    let mut send = SendQ::new(1400);
    for index in 0..64 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    send.flush(0, &peer());
    send.ack_ranges(&[(1, 31), (33, 63)], 1);
    assert_eq!(send.get_sent_queue_size(), 2);
    assert_eq!(send.buffered_bytes, 2 * (2 + SendQ::FRAME_BUDGET));
    let retries = send.flush(1, &peer());
    assert_eq!(
        retries
            .iter()
            .map(|frame| frame.ordered_frame_index)
            .collect::<Vec<_>>(),
        vec![0, 32]
    );
    // Delayed ACKs for the original datagrams release retransmitted frames once.
    send.ack_ranges(&[(0, 63)], 2);
    assert!(send.is_empty());
    assert_eq!(send.buffered_bytes, 0);
    send.ack_ranges(&[(0, 63)], 3);
    assert_eq!(send.buffered_bytes, 0);
}

#[test]
fn batch_ack_preserves_fragment_budget_and_ignores_unsent_ranges() {
    let mut send = SendQ::new(1400);
    send.insert(Reliability::ReliableOrdered, &[0xfe; 4096])
        .unwrap();
    let frames = send.flush(0, &peer());
    let initial = send.buffered_bytes;
    send.ack_ranges(&[(100, 200)], 1);
    assert_eq!(send.buffered_bytes, initial);
    send.ack_ranges(&[(0, 2)], 2);
    assert_eq!(send.buffered_bytes, 4096 + SendQ::FRAME_BUDGET);
    send.ack_ranges(&[(0, frames.last().unwrap().sequence_number)], 3);
    assert_eq!(send.buffered_bytes, 0);
    assert!(send.is_empty());
}

#[test]
fn coalesced_frame_sets_preserve_mtu_indexes_and_ack_all_members() {
    for mtu in [576, 1400, 1492] {
        let mut send = SendQ::new(mtu);
        send.enable_coalescing();
        for index in 0..64 {
            send.insert_with_order_channel(Reliability::ReliableOrdered, &[0xfe, index], 3)
                .unwrap();
        }
        let frames = send.flush(0, &peer());
        let mut receive = RecvQ::new();
        let mut wire = Vec::new();
        let mut begin = 0;
        let mut datagrams = 0;
        while begin < frames.len() {
            let mut end = begin + 1;
            while end < frames.len() && frames[end].sequence_number == frames[begin].sequence_number
            {
                end += 1;
            }
            FrameSetPacket::serialize_group_into(&frames[begin..end], &mut wire).unwrap();
            assert!(wire.len() <= usize::from(mtu) - 28);
            for frame in FrameVec::new(&wire).unwrap().frames {
                receive.insert(frame).unwrap();
            }
            send.ack(frames[begin].sequence_number, 1);
            datagrams += 1;
            begin = end;
        }
        assert!(datagrams < 64);
        let delivered = receive.flush(&peer());
        assert_eq!(delivered.len(), 64);
        for (index, frame) in delivered.iter().enumerate() {
            assert_eq!(frame.data.as_ref(), &[0xfe, index as u8]);
            assert_eq!(frame.order_channel, 3);
            assert_eq!(frame.reliable_frame_index, index as u32);
        }
        assert!(send.is_empty());
        assert_eq!(send.buffered_bytes, 0);
    }
}

#[test]
fn coalesced_loss_retries_and_late_acks_preserve_exactly_once_delivery() {
    for use_nack in [false, true] {
        let mut send = SendQ::new(1400);
        send.enable_coalescing();
        for index in 0..16 {
            send.insert(Reliability::ReliableOrdered, &[0xfe, index])
                .unwrap();
        }
        let initial = send.flush(0, &peer());
        assert_eq!(initial[0].sequence_number, 0);
        assert_eq!(initial[8].sequence_number, 1);
        if use_nack {
            send.nack(0, 1);
            send.nack(1, 1);
        }
        let retry = send.flush(50, &peer());
        assert_eq!(retry.len(), 16);
        let mut receive = RecvQ::new();
        for frame in retry {
            receive.insert(frame).unwrap();
        }
        assert_eq!(receive.flush(&peer()).len(), 16);
        for frame in initial {
            receive.insert(frame).unwrap();
        }
        assert!(receive.flush(&peer()).is_empty());
        send.ack(0, 51);
        send.ack(1, 51);
        assert!(send.is_empty());
        assert_eq!(send.buffered_bytes, 0);
    }
}

#[test]
fn coalescing_respects_wrap_flight_limits_fragments_and_other_modes() {
    let mut send = SendQ::new(1400);
    send.enable_coalescing();
    send.sequence_number = sequence::MASK;
    for index in 0..80 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    let first = send.flush(0, &peer());
    assert_eq!(first.len(), 64);
    assert_eq!(first[0].sequence_number, sequence::MASK);
    assert_eq!(first[8].sequence_number, 0);
    for frames in first.chunks(8) {
        assert!(
            frames
                .iter()
                .all(|frame| frame.sequence_number == frames[0].sequence_number)
        );
        send.ack(frames[0].sequence_number, 1);
    }
    let rest = send.flush(1, &peer());
    assert_eq!(rest.len(), 16);
    for frames in rest.chunks(8) {
        send.ack(frames[0].sequence_number, 2);
    }
    assert!(send.is_empty());
    for mode in [
        Reliability::Unreliable,
        Reliability::UnreliableSequenced,
        Reliability::Reliable,
        Reliability::ReliableSequenced,
    ] {
        let mut send = SendQ::new(1400);
        send.enable_coalescing();
        send.insert(mode, &[0xfe, 0]).unwrap();
        send.insert(mode, &[0xfe, 1]).unwrap();
        let frames = send.flush(0, &peer());
        assert_ne!(frames[0].sequence_number, frames[1].sequence_number);
    }
    let mut send = SendQ::new(1400);
    send.enable_coalescing();
    send.insert(Reliability::ReliableOrdered, &[0xfe; 4096])
        .unwrap();
    let frames = send.flush(0, &peer());
    assert!(frames.iter().all(FrameSetPacket::is_fragment));
    assert!(
        frames
            .windows(2)
            .all(|pair| pair[0].sequence_number != pair[1].sequence_number)
    );
}

#[test]
fn grouped_ack_samples_rtt_once_and_rejects_unsent_sequences() {
    let mut grouped = SendQ::new(1400);
    grouped.enable_coalescing();
    for _ in 0..16 {
        grouped
            .insert(Reliability::ReliableOrdered, &[0xfe])
            .unwrap();
    }
    grouped.flush(0, &peer());
    grouped.ack(99, 1000);
    assert_eq!(grouped.get_sent_queue_size(), 16);
    grouped.ack(0, 1000);
    let mut ordinary = SendQ::new(1400);
    ordinary
        .insert(Reliability::ReliableOrdered, &[0xfe])
        .unwrap();
    ordinary.flush(0, &peer());
    ordinary.ack(0, 1000);
    assert_eq!(grouped.get_rto(), ordinary.get_rto());
    grouped.ack(1, 1000);
    assert!(!grouped.grouped_acks);
}

#[test]
fn later_coalesced_datagram_recovers_a_lost_group_before_the_timeout() {
    let mut send = SendQ::new(1400);
    send.enable_coalescing();
    for index in 0..16 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    let initial = send.flush(0, &peer());
    let mut receive = RecvQ::new();
    for frame in initial.into_iter().skip(8) {
        receive.insert(frame).unwrap();
    }
    assert!(receive.flush(&peer()).is_empty());
    let gaps = receive.get_nack();
    assert_eq!(gaps, vec![(0, 0)]);
    send.nack_ranges(&gaps, 1);
    let retry = send.flush(1, &peer());
    assert_eq!(retry.len(), 8);
    for frame in retry {
        receive.insert(frame).unwrap();
    }
    let delivered = receive.flush(&peer());
    assert_eq!(delivered.len(), 16);
    for (index, frame) in delivered.iter().enumerate() {
        assert_eq!(frame.data.as_ref(), &[0xfe, index as u8]);
    }
    send.ack_ranges(&receive.get_ack(), 2);
    assert!(send.is_empty());
}

#[test]
fn packed_timeout_permanently_restores_individual_datagrams() {
    let mut send = SendQ::new(1400);
    send.enable_coalescing();
    for index in 0..16 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    let first = send.flush(0, &peer());
    assert!(first.coalesced);
    send.flush(SendQ::DEFAULT_TIMEOUT_MILLS, &peer());
    assert!(!send.allows_coalescing());
    send.ack_ranges(&[(0, 1)], 51);
    assert!(send.is_empty());
    send.enable_coalescing();
    for index in 0..16 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    let ordinary = send.flush(52, &peer());
    assert!(!ordinary.coalesced);
    for pair in ordinary.windows(2) {
        assert_ne!(pair[0].sequence_number, pair[1].sequence_number);
    }
}

#[test]
fn loss_feedback_before_batching_keeps_the_ordinary_path() {
    let mut send = SendQ::new(1400);
    send.insert(Reliability::ReliableOrdered, &[0xfe, 0])
        .unwrap();
    send.flush(0, &peer());
    send.nack(99, 1);
    assert!(send.allows_coalescing());
    send.nack(0, 1);
    assert!(!send.allows_coalescing());
    send.flush(1, &peer());
    send.ack(0, 2);
    send.enable_coalescing();
    for index in 0..16 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    let frames = send.flush(3, &peer());
    assert!(!frames.coalesced);
    for pair in frames.windows(2) {
        assert_ne!(pair[0].sequence_number, pair[1].sequence_number);
    }
}

#[test]
fn timeout_before_batching_also_keeps_the_ordinary_path() {
    let mut send = SendQ::new(1400);
    send.insert(Reliability::ReliableOrdered, &[0xfe, 0])
        .unwrap();
    send.flush(0, &peer());
    send.flush(SendQ::DEFAULT_TIMEOUT_MILLS, &peer());
    assert!(!send.allows_coalescing());
    send.ack(0, 51);
    send.enable_coalescing();
    for index in 0..16 {
        send.insert(Reliability::ReliableOrdered, &[0xfe, index])
            .unwrap();
    }
    assert!(!send.flush(52, &peer()).coalesced);
}

#[cfg(feature = "send-policy")]
mod send_policy_tests {
    use super::*;
    #[test]
    fn pure_reliable_sends_keep_the_legacy_window() {
        let mut send = SendQ::new(1400);
        for _ in 0..128 {
            send.insert(Reliability::ReliableOrdered, &[0xfe; 64])
                .unwrap();
        }
        assert_eq!(send.flush(0, &peer()).len(), 64);
        assert!(send.scheduler.is_none());
        assert_eq!(send.get_reliable_queue_size(), 64);
    }

    #[test]
    fn unreliable_messages_bypass_reliable_backpressure_and_flight() {
        let mut send = SendQ::new(1400);
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        for _ in 0..400 {
            send.insert(Reliability::ReliableOrdered, &[0xfe; 800])
                .unwrap();
        }
        assert!(
            !send
                .has_capacity(Reliability::ReliableOrdered, 800)
                .unwrap()
        );
        assert!(send.has_capacity(Reliability::Unreliable, 64).unwrap());
        assert_eq!(send.flush(0, &peer()).len(), 64);
        send.insert(Reliability::Unreliable, &[0xfe; 64]).unwrap();
        let ready = send.flush(1, &peer());
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].reliability().unwrap(), Reliability::Unreliable);
        assert_eq!(send.get_sent_queue_size(), 64);
    }

    #[test]
    fn unreliable_queue_has_its_own_bound_and_recovers_on_drain() {
        let mut send = SendQ::new(1400);
        send.set_send_options(
            crate::SendOptions::default()
                .with_queue_budgets(4096, 2048)
                .unwrap(),
        )
        .unwrap();
        for _ in 0..8 {
            send.insert(Reliability::Unreliable, &[0xfe; 128]).unwrap();
        }
        assert!(!send.has_capacity(Reliability::Unreliable, 128).unwrap());
        assert!(send.has_capacity(Reliability::Reliable, 128).unwrap());
        let ready = send.flush(0, &peer());
        assert_eq!(ready.len(), 8);
        assert!(send.has_capacity(Reliability::Unreliable, 128).unwrap());
        assert_eq!(send.buffered_bytes, 0);
        assert!(send.is_empty());
    }

    #[test]
    fn configured_flight_is_bounded_by_frames_and_bytes() {
        for (frames, bytes, expected) in [(128, 64 * 1024, 128), (128, 2048, 26), (8, 64 * 1024, 8)]
        {
            let mut send = SendQ::new(1400);
            send.set_send_options(
                crate::SendOptions::default()
                    .with_in_flight_limits(frames, bytes)
                    .unwrap(),
            )
            .unwrap();
            for _ in 0..256 {
                send.insert(Reliability::ReliableOrdered, &[0xfe; 64])
                    .unwrap();
            }
            let ready = send.flush(0, &peer());
            assert_eq!(ready.len(), expected);
            let occupied = send
                .sent_packet
                .iter()
                .map(|item| item.0._size().unwrap())
                .sum::<usize>();
            assert!(occupied <= bytes);
            send.ack_ranges(&[(0, 0)], 1);
            assert!(
                send.sent_packet
                    .iter()
                    .map(|item| item.0._size().unwrap())
                    .sum::<usize>()
                    < occupied
            );
            assert_eq!(send.flush(1, &peer()).len(), 1);
        }
    }

    #[test]
    fn packing_still_counts_every_reliable_frame_in_the_window() {
        let mut send = SendQ::new(1400);
        send.set_send_options(
            crate::SendOptions::default()
                .with_in_flight_limits(8, 64 * 1024)
                .unwrap(),
        )
        .unwrap();
        send.enable_coalescing();
        for _ in 0..16 {
            send.insert(Reliability::ReliableOrdered, &[0xfe; 64])
                .unwrap();
        }
        let ready = send.flush(0, &peer());
        assert!(ready.coalesced);
        assert_eq!(ready.len(), 8);
        assert_eq!(send.get_sent_queue_size(), 8);
        send.ack_ranges(&[(ready[0].sequence_number, ready[0].sequence_number)], 1);
        assert!(send.sent_packet.is_empty());
    }

    #[test]
    fn shrinking_a_live_window_preserves_existing_frames() {
        let mut send = SendQ::new(1400);
        for _ in 0..80 {
            send.insert(Reliability::ReliableOrdered, &[0xfe; 64])
                .unwrap();
        }
        send.flush(0, &peer());
        send.set_send_options(
            crate::SendOptions::default()
                .with_in_flight_limits(8, 2048)
                .unwrap(),
        )
        .unwrap();
        assert!(send.flush(1, &peer()).is_empty());
        assert_eq!(send.get_sent_queue_size(), 64);
        send.ack_ranges(&[(0, 63)], 2);
        assert_eq!(send.flush(2, &peer()).len(), 8);
    }

    #[test]
    fn all_three_send_classes_make_progress_without_changing_payloads() {
        let mut send = SendQ::new(1400);
        send.set_send_options(
            crate::SendOptions::default()
                .with_in_flight_limits(128, 64 * 1024)
                .unwrap()
                .with_flush_budget(16 * 1024)
                .unwrap(),
        )
        .unwrap();
        for index in 0..8 {
            send.insert(Reliability::ReliableOrdered, &[0xfe, index])
                .unwrap();
        }
        let initial = send.flush(0, &peer());
        for frame in initial {
            send.nack(frame.sequence_number, 1);
        }
        for index in 8..16 {
            send.insert(Reliability::ReliableOrdered, &[0xfe, index])
                .unwrap();
        }
        for _ in 0..32 {
            send.insert(Reliability::Unreliable, &[0xfe; 800]).unwrap();
        }
        let ready = send.flush(1, &peer());
        assert!(
            ready
                .iter()
                .any(|p| p.reliability().unwrap() == Reliability::Unreliable)
        );
        assert!(ready.iter().any(|p| p.reliable_frame_index < 8
            && p.reliability().unwrap() == Reliability::ReliableOrdered));
        assert!(ready.iter().any(|p| p.reliable_frame_index >= 8
            && p.reliability().unwrap() == Reliability::ReliableOrdered));
        assert!(ready.iter().map(|p| p._size().unwrap()).sum::<usize>() <= 16 * 1024);
        let mut receive = RecvQ::new();
        for frame in ready {
            receive.insert(frame).unwrap();
        }
        for frame in send.flush(2, &peer()) {
            receive.insert(frame).unwrap();
        }
        let delivered = receive.flush(&peer());
        let reliable: Vec<_> = delivered
            .iter()
            .filter(|p| p.data.len() == 2)
            .map(|p| p.data[1])
            .collect();
        assert_eq!(reliable, (0..16).collect::<Vec<_>>());
    }

    #[test]
    fn default_heartbeat_lane_returns_to_the_legacy_path() {
        let mut send = SendQ::new(1400);
        send.insert(Reliability::Unreliable, &[0x00]).unwrap();
        send.flush(0, &peer());
        assert!(send.scheduler.is_none());
        send.insert(Reliability::Reliable, &[0xfe]).unwrap();
        let ready = send.flush(1, &peer());
        send.ack(ready[0].sequence_number, 2);
        assert!(send.is_empty());
        send.insert(Reliability::Unreliable, &[0x03]).unwrap();
        send.flush(3, &peer());
        assert!(send.scheduler.is_none());
    }

    #[test]
    fn queue_reservation_and_options_reject_impossible_limits() {
        assert!(
            crate::SendOptions::default()
                .with_in_flight_limits(0, 2048)
                .is_err()
        );
        assert!(
            crate::SendOptions::default()
                .with_in_flight_limits(4097, 2048)
                .is_err()
        );
        assert!(
            crate::SendOptions::default()
                .with_queue_budgets(usize::MAX, 2048)
                .is_err()
        );
        assert!(crate::SendOptions::default().with_flush_budget(1).is_err());
        let mut send = SendQ::new(1400);
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        assert!(
            send.has_capacity(Reliability::ReliableOrdered, 64 * 1024 * 1024 - 32768)
                .is_err()
        );
        assert!(
            send.has_capacity(Reliability::Unreliable, 128 * 1024)
                .is_err()
        );
    }

    #[test]
    fn initial_connected_reply_precedes_its_unreliable_ping() {
        let mut send = SendQ::new(1400);
        send.insert(Reliability::ReliableOrdered, &[0x13]).unwrap();
        send.insert(Reliability::Unreliable, &[0x00]).unwrap();
        let ready = send.flush(0, &peer());
        assert_eq!(ready[0].data.as_ref(), &[0x13]);
        assert_eq!(ready[1].data.as_ref(), &[0x00]);
    }

    #[test]
    fn configured_ack_gaps_keep_the_original_packing_fallback() {
        let mut send = SendQ::new(1400);
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        send.enable_coalescing();
        for index in 0..16 {
            send.insert(Reliability::ReliableOrdered, &[0xfe, index])
                .unwrap();
        }
        let first = send.flush(0, &peer());
        assert!(first.coalesced);
        send.ack(1, 1);
        assert!(!send.allows_coalescing());
        let retry = send.flush(1, &peer());
        assert_eq!(retry.len(), 8);
        send.ack(0, 2);
        assert!(send.is_empty());
        for index in 16..24 {
            send.insert(Reliability::ReliableOrdered, &[0xfe, index])
                .unwrap();
        }
        let packed = send.flush(3, &peer());
        assert!(!packed.coalesced);
        send.nack(packed[0].sequence_number, 4);
        assert!(!send.allows_coalescing());
    }

    #[test]
    fn scheduled_queues_preserve_all_five_modes_and_fragment_accounting() {
        let mut send = SendQ::new(1400);
        send.set_send_options(
            crate::SendOptions::default()
                .with_in_flight_limits(128, 64 * 1024)
                .unwrap(),
        )
        .unwrap();
        for (mode, channel, data) in [
            (Reliability::Unreliable, 0, vec![0xfe, 0]),
            (Reliability::UnreliableSequenced, 1, vec![0xfe, 1]),
            (Reliability::Reliable, 0, vec![0xfe, 2]),
            (Reliability::ReliableOrdered, 2, {
                let mut d = vec![0xfe; 4096];
                d[1] = 3;
                d
            }),
            (Reliability::ReliableSequenced, 3, vec![0xfe, 4]),
        ] {
            send.insert_with_order_channel(mode, &data, channel)
                .unwrap();
        }
        let mut receive = RecvQ::new();
        for frame in send.flush(0, &peer()) {
            receive.insert(frame).unwrap();
        }
        let delivered = receive.flush(&peer());
        assert_eq!(delivered.len(), 5);
        let mut ids = delivered.iter().map(|p| p.data[1]).collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);
        assert_eq!(
            delivered
                .iter()
                .find(|p| p.data[1] == 3)
                .unwrap()
                .data
                .len(),
            4096
        );
        send.ack_ranges(&receive.get_ack(), 1);
        assert!(send.is_empty());
        assert_eq!(send.buffered_bytes, 0);
        assert!(send.sent_packet.is_empty());
    }

    #[test]
    fn scheduler_resequences_retries_across_datagram_wrap() {
        let mut send = SendQ::new(1400);
        send.sequence_number = sequence::MASK - 1;
        send.set_send_options(
            crate::SendOptions::default()
                .with_in_flight_limits(128, 64 * 1024)
                .unwrap(),
        )
        .unwrap();
        for _ in 0..2 {
            send.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        }
        let initial = send.flush(0, &peer());
        for p in &initial {
            send.nack(p.sequence_number, 1);
        }
        send.insert(Reliability::Unreliable, &[0xfe]).unwrap();
        let retry = send.flush(1, &peer());
        assert_eq!(retry.len(), 3);
        let ids = retry
            .iter()
            .map(|p| p.sequence_number)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 3);
        send.ack_ranges(&[(sequence::MASK - 1, sequence::MASK)], 2);
        assert!(send.is_empty());
        assert!(send.sent_packet.is_empty());
    }

    #[test]
    fn streamed_unreliable_batches_need_not_fit_the_whole_reserve() {
        let mut queue = SendQ::new(1400);
        queue
            .set_send_options(
                crate::SendOptions::default()
                    .with_queue_budgets(4096, 2048)
                    .unwrap(),
            )
            .unwrap();
        assert!(
            !queue
                .has_batch_capacity(Reliability::Unreliable, [64; 100].into_iter())
                .unwrap()
        );
        assert!(queue.has_capacity(Reliability::Unreliable, 64).unwrap());
    }

    #[test]
    fn reliable_capacity_shortcut_respects_custom_and_live_budgets() {
        let mut send = SendQ::new(1400);
        send.set_send_options(
            crate::SendOptions::default()
                .with_queue_budgets(2048, 4096)
                .unwrap(),
        )
        .unwrap();
        for _ in 0..8 {
            send.insert(Reliability::ReliableOrdered, &[0xfe; 128])
                .unwrap();
        }
        assert!(
            !send
                .has_capacity(Reliability::ReliableOrdered, 128)
                .unwrap()
        );
        assert!(send.has_capacity(Reliability::Unreliable, 128).unwrap());
        send.insert(Reliability::Unreliable, &[0xfe; 128]).unwrap();
        assert!(
            !send
                .has_capacity(Reliability::ReliableOrdered, 128)
                .unwrap()
        );
        let ready = send.flush(0, &peer());
        let ids = ready
            .iter()
            .filter(|p| p.is_reliable().unwrap())
            .map(|p| p.sequence_number)
            .collect::<Vec<_>>();
        for id in ids {
            send.ack(id, 1);
        }
        assert!(
            send.has_capacity(Reliability::ReliableOrdered, 128)
                .unwrap()
        );
        assert!(
            send.has_capacity(Reliability::ReliableOrdered, 4096)
                .unwrap()
        );
        assert!(
            send.has_capacity(Reliability::ReliableOrdered, 64 * 1024 * 1024 - 4096)
                .is_err()
        );
    }

    #[test]
    fn application_unreliable_traffic_keeps_its_idle_reservation() {
        let mut send = SendQ::new(1400);
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        send.insert(Reliability::Unreliable, &[0xfe]).unwrap();
        send.flush(0, &peer());
        assert!(send.scheduler.as_ref().unwrap().persistent);
        assert!(
            send.has_capacity(Reliability::ReliableOrdered, 64 * 1024 * 1024 - 32768)
                .is_err()
        );
        send.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        let ready = send.flush(1, &peer());
        assert_eq!(send.sent_packet.len(), 1);
        send.ack(ready[0].sequence_number, 2);
        assert!(send.is_empty());
        assert!(send.scheduler.is_some());
    }

    #[test]
    fn explicitly_configured_reserve_survives_control_drain() {
        let mut send = SendQ::new(1400);
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        send.insert(Reliability::Unreliable, &[0x00]).unwrap();
        send.flush(0, &peer());
        assert!(send.scheduler.as_ref().unwrap().persistent);
        assert_eq!(send.send_options(), Some(crate::SendOptions::default()));
    }

    #[test]
    fn fused_batch_validation_handles_fit_fragments_and_streamed_modes() {
        let send = SendQ::new(1400);
        for (lengths, expected) in [
            (vec![800, 800], false),
            (vec![600, 600], true),
            (vec![800, 64, 800], true),
            (vec![1341, 1], false),
            (vec![4096, 64], false),
        ] {
            assert_eq!(
                send.batch_can_coalesce(Reliability::ReliableOrdered, lengths.into_iter())
                    .unwrap(),
                expected
            );
        }
        for mode in [
            Reliability::Unreliable,
            Reliability::UnreliableSequenced,
            Reliability::Reliable,
            Reliability::ReliableSequenced,
        ] {
            assert!(
                !send
                    .batch_can_coalesce(mode, [128, 128].into_iter())
                    .unwrap()
            );
            assert!(
                send.batch_can_coalesce(mode, [64, 1400].into_iter())
                    .is_err()
            );
        }
        assert!(
            !send
                .batch_can_coalesce(Reliability::Unreliable, std::iter::repeat_n(64, 1000))
                .unwrap()
        );
        assert!(
            send.batch_can_coalesce(Reliability::ReliableOrdered, [usize::MAX].into_iter())
                .is_err()
        );
    }

    #[test]
    fn configured_admission_keeps_the_unreliable_reserve_at_the_accounting_cap() {
        let mut queue = SendQ::new(1400);
        queue
            .set_send_options(crate::SendOptions::default())
            .unwrap();
        let message_budget = 800 + SendQ::FRAME_BUDGET;
        let count = (SendQ::MAX_BUFFERED_BYTES - 1024) / message_budget;
        assert!(
            queue
                .has_batch_capacity(
                    Reliability::ReliableOrdered,
                    std::iter::repeat_n(800, count),
                )
                .is_err()
        );
    }

    #[tokio::test]
    async fn unreliable_queue_release_wakes_capacity_waiters_without_task_state() {
        let capacity = Arc::new(tokio::sync::Notify::new());
        let available = capacity.notified();
        tokio::pin!(available);
        available.as_mut().enable();
        let mut queue = SendQ::new(1400);
        queue
            .set_send_options(crate::SendOptions::default())
            .unwrap();
        queue.register_capacity_notifier(&capacity);
        queue.insert(Reliability::Unreliable, &[0xfe; 64]).unwrap();
        assert_eq!(queue.flush(0, &peer()).len(), 1);
        tokio::time::timeout(std::time::Duration::from_millis(20), available)
            .await
            .expect("unreliable capacity waiter was not notified");
        assert_eq!(Arc::strong_count(&capacity), 1);
        drop(queue);
        assert_eq!(Arc::weak_count(&capacity), 0);
    }

    #[test]
    fn unconfigured_unreliable_traffic_uses_the_original_queue_policy() {
        let mut send = SendQ::new(1400);
        assert_eq!(send.send_options(), None);
        send.insert(Reliability::Unreliable, &[0x00]).unwrap();
        send.insert(Reliability::Unreliable, &[0xfe]).unwrap();
        assert!(send.scheduler.is_none());
        assert_eq!(send.queued_unreliable, 2);
        assert_eq!(send.flush(0, &peer()).len(), 2);
        assert_eq!(send.queued_unreliable, 0);
        assert!(send.is_empty());
    }

    #[test]
    fn explicit_activation_migrates_both_pending_classes_without_dropping_data() {
        let mut send = SendQ::new(1400);
        for (mode, data) in [
            (Reliability::ReliableOrdered, [0xfe, 0]),
            (Reliability::Unreliable, [0xfe, 2]),
            (Reliability::ReliableOrdered, [0xfe, 1]),
            (Reliability::Unreliable, [0xfe, 3]),
        ] {
            send.insert(mode, &data).unwrap();
        }
        let reserved = send.buffered_bytes;
        send.set_send_options(crate::SendOptions::default())
            .unwrap();
        assert_eq!(send.buffered_bytes, reserved);
        assert_eq!(send.queued_unreliable, 0);
        assert_eq!(send.packets.len(), 2);
        assert_eq!(send.scheduler.as_ref().unwrap().unreliable.len(), 2);
        let ready = send.flush(0, &peer());
        let mut received = ready.iter().map(|frame| frame.data[1]).collect::<Vec<_>>();
        received.sort_unstable();
        assert_eq!(received, [0, 1, 2, 3]);
        for frame in &ready {
            send.ack(frame.sequence_number, 1);
        }
        assert!(send.is_empty());
        assert_eq!(send.buffered_bytes, 0);
    }

    #[test]
    fn unconfigured_ack_gaps_keep_the_original_packing_fallback() {
        let mut send = SendQ::new(1400);
        send.enable_coalescing();
        for _ in 0..16 {
            send.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        }
        assert!(send.flush(0, &peer()).coalesced);
        send.ack(1, 1);
        assert!(!send.allows_coalescing());
        assert_eq!(send.flush(1, &peer()).len(), 8);
        send.ack(0, 2);
        assert!(send.is_empty());
    }

    #[test]
    fn large_pending_unreliable_counts_never_reject_or_lose_frames() {
        let mut send = SendQ::new(1400);
        let count = usize::from(u16::MAX) + 2;
        for _ in 0..count {
            send.insert(Reliability::Unreliable, &[0xfe]).unwrap();
        }
        assert_eq!(send.queued_unreliable, count);
        assert_eq!(send.flush(0, &peer()).len(), count);
        assert_eq!(send.queued_unreliable, 0);
        assert_eq!(send.buffered_bytes, 0);
        assert!(send.is_empty());
    }
}

#[cfg(feature = "recovery-policy")]
mod recovery_tests {
    use super::*;
    fn probe_queue(minimum: u64, backoff: bool) -> SendQ {
        let mut q = SendQ::new(1400);
        q.set_recovery_options(
            crate::RecoveryOptions {
                tail_probe_min_delay: Some(std::time::Duration::from_millis(minimum)),
                reset_backoff_on_progress: backoff,
                ..Default::default()
            },
            0,
        );
        // Warm with unambiguous feedback; keep the legacy estimator and floor.
        for i in 0..40 {
            q.insert(Reliability::ReliableOrdered, &[0xfe, i]).unwrap();
            let frames = q.flush(i64::from(i) * 2, &peer());
            q.ack(frames[0].sequence_number, i64::from(i) * 2 + 1);
        }
        q
    }

    #[test]
    fn tail_probe_is_disabled_by_default_and_requires_rtt_feedback() {
        let mut q = SendQ::new(1400);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 1]).unwrap();
        q.flush(0, &peer());
        assert!(q.flush(10, &peer()).is_empty());
        q.set_recovery_options(
            crate::RecoveryOptions {
                tail_probe_min_delay: Some(std::time::Duration::from_millis(5)),
                ..Default::default()
            },
            0,
        );
        assert!(q.flush(10, &peer()).is_empty());
        assert_eq!(q.recovery_deadline(), 50);
    }

    #[test]
    fn lost_tail_is_recovered_once_without_waiting_for_base_rto() {
        let mut q = probe_queue(5, false);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 42]).unwrap();
        let original = q.flush(100, &peer())[0].clone();
        assert_eq!(q.recovery_deadline(), 105);
        assert!(q.flush(104, &peer()).is_empty());
        let retry = q.flush(105, &peer());
        assert_eq!(retry.len(), 1);
        assert_ne!(retry[0].sequence_number, original.sequence_number);
        assert_eq!(retry[0].reliable_frame_index, original.reliable_frame_index);
        assert!(q.flush(110, &peer()).is_empty());
        q.ack(original.sequence_number, 111);
        assert!(q.is_empty());
        assert_eq!(q.recovery_deadline(), i64::MAX);
    }

    #[test]
    fn lost_ack_probe_preserves_exactly_once_delivery_and_karn() {
        let mut q = probe_queue(10, false);
        let mut recv = RecvQ::new();
        // This receiver starts after the training messages.
        recv.reliable_window.next = q.reliable_frame_index;
        recv.last_ordered_indexes
            .insert(0, q.ordered_frame_indexes.get(0));
        q.insert(Reliability::ReliableOrdered, &[0xfe, 42]).unwrap();
        let original = q.flush(100, &peer())[0].clone();
        recv.insert(original).unwrap();
        assert_eq!(recv.flush(&peer()).len(), 1);
        let old_rto = q.rto;
        let retry = q.flush(110, &peer())[0].clone();
        recv.insert(retry.clone()).unwrap();
        assert!(recv.flush(&peer()).is_empty());
        q.ack(retry.sequence_number, 111);
        assert!(q.is_empty());
        assert_eq!(q.rto, old_rto);
    }

    #[test]
    fn probe_replays_a_complete_packed_datagram_and_keeps_packing_available() {
        let mut q = probe_queue(5, false);
        q.enable_coalescing();
        for i in 0..8 {
            q.insert(Reliability::ReliableOrdered, &[0xfe, i]).unwrap();
        }
        assert_eq!(q.flush(100, &peer()).len(), 8);
        let replay = q.flush(105, &peer());
        assert_eq!(replay.len(), 8);
        assert!(replay.coalesced);
        assert!(
            replay
                .iter()
                .all(|p| p.sequence_number == replay[0].sequence_number)
        );
        assert!(q.allows_coalescing());
        assert!(q.flush(106, &peer()).is_empty());
        // Normal timeout still recovers the remaining frames and disables packing.
        assert!(!q.flush(180, &peer()).is_empty());
        assert!(!q.allows_coalescing());
    }

    #[test]
    fn stale_or_duplicate_acks_do_not_rearm_probe() {
        let mut q = probe_queue(5, false);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 1]).unwrap();
        q.flush(100, &peer());
        q.flush(105, &peer());
        q.ack(sequence::MASK, 106);
        assert!(q.flush(110, &peer()).is_empty());
        q.ack_ranges(&[(sequence::MASK, sequence::MASK)], 111);
        assert!(q.flush(116, &peer()).is_empty());
    }

    #[test]
    fn fresh_flight_after_idle_gets_a_fresh_probe_deadline() {
        let mut q = probe_queue(5, false);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 1]).unwrap();
        q.flush(10000, &peer());
        assert_eq!(q.probe_deadline(), 10005);
        assert!(q.flush(10004, &peer()).is_empty());
    }

    #[test]
    fn disable_probe_cancels_early_deadline_without_discarding_data() {
        let mut q = probe_queue(5, false);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 1]).unwrap();
        q.flush(100, &peer());
        q.set_recovery_options(Default::default(), 101);
        assert_eq!(q.recovery_deadline(), 150);
        assert!(q.flush(105, &peer()).is_empty());
        assert_eq!(q.flush(150, &peer()).len(), 1);
    }

    #[test]
    fn ack_progress_caps_backoff_without_allowing_ambiguous_rtt_samples() {
        let mut q = probe_queue(5, true);
        for i in 0..2 {
            q.insert(Reliability::ReliableOrdered, &[0xfe, i]).unwrap();
        }
        let frames = q.flush(100, &peer());
        q.sent_packet[1].3 = 8;
        q.ack(frames[0].sequence_number, 101);
        assert_eq!(q.sent_packet[0].3, 1);
        let rto = q.rto;
        q.ack(frames[1].sequence_number, 102);
        assert_eq!(q.rto, rto);
    }

    #[test]
    fn delayed_feedback_does_not_cause_a_probe_storm() {
        let mut q = probe_queue(5, false);
        q.insert(Reliability::ReliableOrdered, &[0xfe, 1]).unwrap();
        q.flush(100, &peer());
        let mut retries = 0;
        for now in 101..900 {
            retries += q.flush(now, &peer()).len();
        }
        assert!(retries > 1 && retries < 10, "retries: {retries}");
    }

    #[test]
    fn early_probe_does_not_compete_with_a_full_flight() {
        let mut queue = probe_queue(5, false);
        for i in 0..16 {
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe, i])
                .unwrap();
        }
        queue.flush(100, &peer());
        assert_eq!(queue.probe_deadline(), i64::MAX);
        assert!(queue.flush(105, &peer()).is_empty());
        assert_eq!(queue.flush(150, &peer()).len(), 16);
    }

    #[test]
    fn ambiguous_ack_progress_does_not_reset_backoff() {
        let mut queue = probe_queue(5, true);
        for i in 0..2 {
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe, i])
                .unwrap();
        }
        let frames = queue.flush(100, &peer());
        queue.sent_packet[0].3 = 1;
        queue.sent_packet[1].3 = 8;
        let previous = queue.rto;
        queue.ack(frames[0].sequence_number, 900);
        assert_eq!(queue.sent_packet[0].3, 8);
        assert_eq!(queue.rto, previous);
    }

    #[test]
    fn packed_tail_probe_survives_lost_datagram_or_ack_without_duplicate_delivery() {
        for original_lost in [false, true] {
            let mut queue = probe_queue(5, false);
            let mut recv = RecvQ::new();
            recv.reliable_window.next = queue.reliable_frame_index;
            recv.last_ordered_indexes
                .insert(0, queue.ordered_frame_indexes.get(0));
            recv.sequence_number_ackset.next_expected = queue.sequence_number;
            queue.enable_coalescing();
            for id in 0..8 {
                queue
                    .insert(Reliability::ReliableOrdered, &[0xfe, id])
                    .unwrap();
            }
            let original = queue.flush(100, &peer());
            let mut wire = Vec::new();
            if !original_lost {
                FrameSetPacket::serialize_group_into(&original, &mut wire).unwrap();
                for frame in FrameVec::new(&wire).unwrap().frames {
                    recv.insert(frame).unwrap();
                }
                assert_eq!(recv.flush(&peer()).len(), 8);
                recv.get_ack(); // The ACK is lost; the sender still owns the whole datagram.
            }
            let replay = queue.flush(105, &peer());
            FrameSetPacket::serialize_group_into(&replay, &mut wire).unwrap();
            for frame in FrameVec::new(&wire).unwrap().frames {
                recv.insert(frame).unwrap();
            }
            assert_eq!(recv.flush(&peer()).len(), if original_lost { 8 } else { 0 });
            queue.ack_ranges(&recv.get_ack(), 106);
            assert!(queue.is_empty());
            assert_eq!(queue.buffered_bytes, 0);
            assert!(queue.allows_coalescing());
        }
    }

    #[cfg(feature = "send-policy")]
    #[test]
    fn scheduled_probe_replays_every_packed_frame_in_one_flush() {
        let mut queue = probe_queue(5, false);
        queue
            .set_send_options(
                crate::SendOptions::default()
                    .with_flush_budget(2048)
                    .unwrap(),
            )
            .unwrap();
        queue.enable_coalescing();
        for id in 0..8 {
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe, id])
                .unwrap();
        }
        let original = queue.flush(100, &peer());
        assert!(original.coalesced);
        let replay = queue.flush(105, &peer());
        assert!(replay.coalesced);
        assert_eq!(replay.len(), 8);
        assert!(
            replay
                .iter()
                .all(|frame| frame.sequence_number == replay[0].sequence_number)
        );
        queue.ack(replay[0].sequence_number, 106);
        assert!(queue.is_empty());
    }

    #[test]
    fn packed_tail_probe_and_late_acks_cross_sequence_wrap() {
        for original_ack in [true, false] {
            let mut queue = probe_queue(5, false);
            queue.sequence_number = sequence::MASK;
            queue.ack_sequence_number = sequence::MASK - 1;
            queue.enable_coalescing();
            for id in 0..8 {
                queue
                    .insert(Reliability::ReliableOrdered, &[0xfe, id])
                    .unwrap();
            }
            let original = queue.flush(100, &peer());
            assert_eq!(original[0].sequence_number, sequence::MASK);
            let replay = queue.flush(105, &peer());
            assert_eq!(replay[0].sequence_number, 0);
            queue.ack(if original_ack { sequence::MASK } else { 0 }, 106);
            assert!(queue.is_empty());
            assert_eq!(queue.buffered_bytes, 0);
            assert!(queue.allows_coalescing());
        }
    }

    #[test]
    fn late_maintenance_preserves_normal_rto_before_tail_probing() {
        for now in [150, 300] {
            let mut queue = probe_queue(10, false);
            queue.enable_coalescing();
            for id in 0..8 {
                queue
                    .insert(Reliability::ReliableOrdered, &[0xfe, id])
                    .unwrap();
            }
            queue.flush(100, &peer());
            let retry = queue.flush(now, &peer());
            assert_eq!(retry.len(), 8);
            assert!(!retry.coalesced);
            assert!(!queue.allows_coalescing());
            let unique: std::collections::HashSet<_> =
                retry.iter().map(|p| p.sequence_number).collect();
            assert_eq!(unique.len(), 8);
        }
    }
    #[test]
    fn a_lost_probe_does_not_delay_normal_rto_or_add_backoff() {
        for packed in [false, true] {
            let mut queue = probe_queue(10, false);
            if packed {
                queue.enable_coalescing();
            }
            let count = if packed { 8 } else { 1 };
            for id in 0..count {
                queue
                    .insert(Reliability::ReliableOrdered, &[0xfe, id])
                    .unwrap();
            }
            queue.flush(100, &peer());
            let probe = queue.flush(110, &peer());
            assert_eq!(probe.len(), usize::from(count));
            assert!(
                queue
                    .sent_packet
                    .iter()
                    .all(|p| p.3 == 0 && !p.4.is_empty())
            );
            assert_eq!(queue.recovery_deadline(), 150);
            assert!(queue.flush(149, &peer()).is_empty());
            let retry = queue.flush(150, &peer());
            assert_eq!(retry.len(), usize::from(count));
            assert!(!retry.coalesced);
            assert!(queue.sent_packet.iter().all(|p| p.3 == 1));
            assert_eq!(queue.recovery_deadline(), 225);
            assert!(queue.flush(151, &peer()).is_empty());
        }
    }

    #[test]
    fn late_normal_retry_is_not_immediately_probed_again() {
        let mut queue = probe_queue(10, false);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &peer());
        assert_eq!(queue.flush(150, &peer()).len(), 1);
        assert!(queue.flush(151, &peer()).is_empty());
        assert_eq!(queue.recovery_deadline(), 225);
    }

    #[test]
    fn probe_fallback_only_retries_its_original_datagram() {
        let mut queue = probe_queue(10, false);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &peer());
        queue.flush(110, &peer());
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 2])
            .unwrap();
        let fresh = queue.flush(120, &peer())[0].clone();
        let retry = queue.flush(150, &peer());
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].data.as_ref(), [0xfe, 1]);
        assert!(
            queue
                .sent_packet
                .iter()
                .any(|p| p.0.sequence_number == fresh.sequence_number && p.3 == 0)
        );
    }

    #[test]
    fn disabling_probes_preserves_an_existing_fallback_deadline() {
        let mut queue = probe_queue(10, false);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &peer());
        queue.flush(110, &peer());
        queue.set_recovery_options(crate::RecoveryOptions::default(), 115);
        assert_eq!(queue.recovery_options(), crate::RecoveryOptions::default());
        assert_eq!(queue.recovery_deadline(), 150);
        assert!(queue.flush(149, &peer()).is_empty());
        let retry = queue.flush(150, &peer());
        assert_eq!(retry.len(), 1);
        assert_eq!(queue.recovery_deadline(), 225);
        queue.ack(retry[0].sequence_number, 151);
        queue.flush(151, &peer());
        assert!(queue.recovery.is_none());
    }
    #[test]
    fn retiring_a_probe_clears_its_deadline_even_with_a_busy_flight() {
        let mut queue = probe_queue(10, false);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &peer());
        let probe = queue.flush(110, &peer())[0].clone();
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 2])
            .unwrap();
        queue.flush(120, &peer());
        queue.ack(probe.sequence_number, 125);
        assert_eq!(queue.sent_packet.len(), 1);
        assert!(queue.recovery.as_ref().unwrap().pending_probe.is_none());
        assert_eq!(queue.probe_retry_deadline(), i64::MAX);
    }
    #[test]
    fn nack_with_a_reused_datagram_id_still_adds_normal_backoff() {
        let mut queue = probe_queue(10, false);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &peer());
        let probe = queue.flush(110, &peer())[0].clone();
        queue.sequence_number = probe.sequence_number;
        queue.nack(probe.sequence_number, 115);
        assert_eq!(queue.flush(115, &peer()).len(), 1);
        assert_eq!(queue.sent_packet[0].3, 1);
        assert_eq!(queue.probe_retry_deadline(), i64::MAX);
    }
}
