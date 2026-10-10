use hangar_server::costs::fx::Fx;
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

#[test]
fn first_call_waits_and_caches_success_for_exactly_one_hour() {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(AtomicU64::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let fetch_calls = calls.clone();
    let fetch_clock = clock.clone();
    let fx = Arc::new(Fx::with_fetch_and_clock(move || {
        fetch_calls.fetch_add(1, Ordering::SeqCst);
        started_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        Some(5.25)
    }, move || Duration::from_secs(fetch_clock.load(Ordering::SeqCst))));
    let fx_first = fx.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let first = std::thread::spawn(move || result_tx.send(fx_first.usd_brl()).unwrap());
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(result_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert_eq!(result_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Some(5.25));
    first.join().unwrap();
    clock.store(3599, Ordering::SeqCst);
    assert_eq!(fx.usd_brl(), Some(5.25));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    clock.store(3600, Ordering::SeqCst);
    assert_eq!(fx.usd_brl(), Some(5.25));
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    release_tx.send(()).unwrap();
}

#[test]
fn failed_first_attempt_is_cached_and_next_attempt_runs_in_background() {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(AtomicU64::new(0));
    let fetch_calls = calls.clone();
    let fetch_clock = clock.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let fx = Fx::with_fetch_and_clock(move || {
        if fetch_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return None;
        }
        started_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        Some(5.5)
    }, move || Duration::from_secs(fetch_clock.load(Ordering::SeqCst)));
    assert_eq!(fx.usd_brl(), None);
    assert_eq!(fx.usd_brl(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    clock.store(3600, Ordering::SeqCst);
    assert_eq!(fx.usd_brl(), None);
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..10 { assert_eq!(fx.usd_brl(), None); }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fx.usd_brl() != Some(5.5) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn stale_value_survives_a_failed_refresh_and_failure_renews_ttl() {
    let clock = Arc::new(AtomicU64::new(100));
    let fetch_clock = clock.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let fetch_calls = calls.clone();
    let (finished_tx, finished_rx) = mpsc::channel();
    let fx = Fx::with_fetch_and_clock(move || {
        let number = fetch_calls.fetch_add(1, Ordering::SeqCst);
        finished_tx.send(number).unwrap();
        if number == 0 { Some(5.25) } else { None }
    }, move || Duration::from_secs(fetch_clock.load(Ordering::SeqCst)));
    assert_eq!(fx.usd_brl(), Some(5.25));
    assert_eq!(finished_rx.recv().unwrap(), 0);
    clock.store(3700, Ordering::SeqCst);
    assert_eq!(fx.usd_brl(), Some(5.25));
    assert_eq!(finished_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    clock.store(7299, Ordering::SeqCst);
    assert_eq!(fx.usd_brl(), Some(5.25));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn injected_fetch_does_not_need_an_async_runtime_or_network() {
    let fx = Fx::with_fetch(|| Some(5.25));
    assert_eq!(fx.usd_brl(), Some(5.25));
    assert_eq!(fx.usd_brl(), Some(5.25));
    let _default = Fx::default();
}

#[test]
fn panic_does_not_turn_the_next_attempt_into_a_null_success() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let calls = std::sync::Arc::new(calls);
    let count = calls.clone();
    let fx = Fx::with_fetch(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("falha sintética");
    });
    for _ in 0..4 {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.usd_brl())).is_err());
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
}
