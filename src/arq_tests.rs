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
