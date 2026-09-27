//! Handles, events and ports.

#[test]
fn event_signal_and_wait_one() {
    let event = zx::Event::create();
    assert_eq!(
        event.wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE_PAST).to_result(),
        Err(zx::Status::TIMED_OUT)
    );
    event.signal(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED).unwrap();
    assert_eq!(
        event.wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE).to_result(),
        Ok(zx::Signals::EVENT_SIGNALED)
    );
}

#[test]
fn wait_one_wakes_on_signal_from_another_thread() {
    let event = zx::Event::create();
    let other = event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    let signaler = std::thread::spawn(move || {
        other.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
    });
    assert_eq!(
        event.wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE).to_result(),
        Ok(zx::Signals::USER_0)
    );
    signaler.join().unwrap();
}

#[test]
fn handle_duplicate_replace_and_close() {
    let event = zx::Event::create();
    let koid = event.koid().unwrap();
    let duplicate = event.duplicate_handle(zx::Rights::WAIT).unwrap();
    assert_eq!(duplicate.koid().unwrap(), koid);
    assert_eq!(duplicate.basic_info().unwrap().rights, zx::Rights::WAIT);
    assert_eq!(event.count_info().unwrap().handle_count, 2);
    // Rights can only be reduced.
    assert_eq!(duplicate.duplicate_handle(zx::Rights::WAIT), Err(zx::Status::ACCESS_DENIED));
    let replaced = event.replace_handle(zx::Rights::SIGNAL | zx::Rights::INSPECT).unwrap();
    assert_eq!(replaced.koid().unwrap(), koid);
    drop(duplicate);
    assert_eq!(replaced.count_info().unwrap().handle_count, 1);
}

#[test]
fn port_queue_and_wait() {
    let port = zx::Port::create();
    let packet = zx::Packet::from_user_packet(42, 7, zx::UserPacket::from_u8_array([3; 32]));
    port.queue(&packet).unwrap();
    let received = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(received.key(), 42);
    assert_eq!(received.status(), 7);
    assert_eq!(received.contents(), zx::PacketContents::User(zx::UserPacket::from_u8_array([3; 32])));
    assert_eq!(port.wait(zx::MonotonicInstant::INFINITE_PAST), Err(zx::Status::TIMED_OUT));
}

#[test]
fn wait_async_queues_signal_packet() {
    let port = zx::Port::create();
    let event = zx::Event::create();
    event.wait_async(&port, 5, zx::Signals::USER_1, zx::WaitAsyncOpts::empty()).unwrap();
    assert_eq!(port.wait(zx::MonotonicInstant::INFINITE_PAST), Err(zx::Status::TIMED_OUT));
    event.signal(zx::Signals::NONE, zx::Signals::USER_1).unwrap();
    let packet = port.wait(zx::MonotonicInstant::INFINITE).unwrap();
    assert_eq!(packet.key(), 5);
    let zx::PacketContents::SignalOne(signal) = packet.contents() else {
        panic!("expected a signal packet, got {:?}", packet.contents());
    };
    assert_eq!(signal.trigger(), zx::Signals::USER_1);
    assert_eq!(signal.observed(), zx::Signals::USER_1);
}

#[test]
fn port_cancel_removes_wait() {
    let port = zx::Port::create();
    let event = zx::Event::create();
    event.wait_async(&port, 9, zx::Signals::USER_2, zx::WaitAsyncOpts::empty()).unwrap();
    port.cancel(9).unwrap();
    event.signal(zx::Signals::NONE, zx::Signals::USER_2).unwrap();
    assert_eq!(port.wait(zx::MonotonicInstant::INFINITE_PAST), Err(zx::Status::TIMED_OUT));
}

#[test]
fn closing_handle_cancels_async_wait() {
    let port = zx::Port::create();
    let event = zx::Event::create();
    let duplicate = event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
    duplicate.wait_async(&port, 3, zx::Signals::USER_0, zx::WaitAsyncOpts::empty()).unwrap();
    // Closing the handle the wait was made through cancels it.
    drop(duplicate);
    event.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
    assert_eq!(port.wait(zx::MonotonicInstant::INFINITE_PAST), Err(zx::Status::TIMED_OUT));
}

#[test]
fn page_size_is_host_page_size() {
    let expected = if cfg!(all(target_os = "macos", target_arch = "aarch64")) { 16384 } else { 4096 };
    assert_eq!(zx::system_get_page_size(), expected);
}
