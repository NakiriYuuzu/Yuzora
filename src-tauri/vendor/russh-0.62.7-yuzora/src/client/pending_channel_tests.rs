// Yuzora cancellation regressions, 2026-10-03. Licensed under Apache-2.0.
use super::*;

fn confirmation(id: u32) -> ChannelMsg {
    ChannelMsg::Open {
        id: ChannelId(id),
        max_packet_size: 32768,
        window_size: 65536,
    }
}

#[test]
fn unconfirmed_drop_closes_receiver_without_actor_message() {
    let (sender, receiver) = channel(1);
    let (cancel_sender, mut cancelled) = unbounded_channel();
    drop(ChannelOpenReceiver {
        receiver: Some(receiver),
        cancel_sender: &cancel_sender,
        confirmed_id: None,
    });
    assert!(sender.is_closed());
    assert!(cancelled.try_recv().is_err());
}

#[test]
fn queued_confirmation_closes_its_exact_channel() {
    let (sender, receiver) = channel(1);
    let (cancel_sender, mut cancelled) = unbounded_channel();
    sender.try_send(confirmation(7)).unwrap();
    drop(ChannelOpenReceiver {
        receiver: Some(receiver),
        cancel_sender: &cancel_sender,
        confirmed_id: None,
    });
    assert!(sender.is_closed());
    assert!(matches!(
        cancelled.try_recv().unwrap(),
        Msg::Channel(ChannelId(7), ChannelMsg::Close)
    ));
    assert!(cancelled.try_recv().is_err());
}

#[test]
fn consumed_confirmation_retains_close_ownership() {
    let (_sender, receiver) = channel(1);
    let (cancel_sender, mut cancelled) = unbounded_channel();
    drop(ChannelOpenReceiver {
        receiver: Some(receiver),
        cancel_sender: &cancel_sender,
        confirmed_id: Some(ChannelId(11)),
    });
    assert!(matches!(
        cancelled.try_recv().unwrap(),
        Msg::Channel(ChannelId(11), ChannelMsg::Close)
    ));
}

#[test]
fn reserved_send_after_cancellation_observes_closed_receiver() {
    let (sender, receiver) = channel(1);
    let (cancel_sender, mut cancelled) = unbounded_channel();
    let reserved = sender.try_reserve().unwrap();
    drop(ChannelOpenReceiver {
        receiver: Some(receiver),
        cancel_sender: &cancel_sender,
        confirmed_id: None,
    });
    // Sender::send can already own a permit when the open future is dropped.
    // Its enqueue succeeds, so the session must check is_closed after send.
    reserved.send(confirmation(13));
    assert!(sender.is_closed());
    assert!(cancelled.try_recv().is_err());
}

#[test]
fn successful_handoff_keeps_receiver_open() {
    let (sender, receiver) = channel(1);
    let (cancel_sender, mut cancelled) = unbounded_channel();
    let mut pending = ChannelOpenReceiver {
        receiver: Some(receiver),
        cancel_sender: &cancel_sender,
        confirmed_id: Some(ChannelId(17)),
    };
    let mut receiver = pending.receiver.take().unwrap();
    drop(pending);
    assert!(!sender.is_closed());
    sender.try_send(ChannelMsg::Eof).unwrap();
    assert!(matches!(receiver.try_recv().unwrap(), ChannelMsg::Eof));
    assert!(cancelled.try_recv().is_err());
}
