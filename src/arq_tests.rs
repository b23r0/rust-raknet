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
