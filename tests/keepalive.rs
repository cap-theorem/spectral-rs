mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use common::{FakeSidecar, Reply};
use spectral_sdk::{Error, Lease};

fn lease(ttl: Duration) -> Lease {
    Lease {
        token: "abc123".to_string(),
        ttl,
        expires_at: SystemTime::now() + ttl,
    }
}

fn wait_for(condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "condition never became true"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn renews_until_stopped() {
    let renews = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renews);
    let fake = FakeSidecar::start(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reply::Empty(202)
    });

    let keeper = fake
        .client()
        .keep_alive(&lease(Duration::from_millis(60)))
        .unwrap();
    wait_for(|| renews.load(Ordering::SeqCst) >= 3);

    assert!(!keeper.is_finished());
    keeper.stop().unwrap();
}

#[test]
fn stops_when_lease_rejected() {
    let fake = FakeSidecar::start(|_| Reply::Text(404, "unknown lease"));
    let keeper = fake
        .client()
        .keep_alive(&lease(Duration::from_millis(60)))
        .unwrap();

    wait_for(|| keeper.is_finished());
    let err = keeper.stop().unwrap_err();
    assert!(matches!(err, Error::LeaseExpired(Some(_))), "{err:?}");
}

#[test]
fn gives_up_after_ttl() {
    let fake = FakeSidecar::start(|_| Reply::Text(500, "boom"));
    let start = Instant::now();
    let keeper = fake
        .client()
        .keep_alive(&lease(Duration::from_millis(60)))
        .unwrap();

    wait_for(|| keeper.is_finished());
    assert!(start.elapsed() >= Duration::from_millis(60));

    let err = keeper.stop().unwrap_err();
    assert!(matches!(err, Error::LeaseExpired(Some(_))), "{err:?}");
    assert_eq!(err.api().map(|api| api.status), Some(500));
}

#[test]
fn retries_transient_failure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let fake = FakeSidecar::start(move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Text(500, "blip")
        } else {
            Reply::Empty(202)
        }
    });

    let keeper = fake
        .client()
        .keep_alive(&lease(Duration::from_millis(60)))
        .unwrap();
    wait_for(|| calls.load(Ordering::SeqCst) >= 3);

    assert!(!keeper.is_finished());
    keeper.stop().unwrap();
}

#[test]
fn dropping_handle_stops_renewing() {
    let renews = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renews);
    let fake = FakeSidecar::start(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reply::Empty(202)
    });

    let keeper = fake
        .client()
        .keep_alive(&lease(Duration::from_millis(30)))
        .unwrap();
    wait_for(|| renews.load(Ordering::SeqCst) >= 1);
    drop(keeper);

    std::thread::sleep(Duration::from_millis(50));
    let after_drop = renews.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(renews.load(Ordering::SeqCst), after_drop);
}

#[test]
fn rejects_ttl_too_short() {
    let fake = FakeSidecar::start(|_| Reply::Empty(202));
    let result = fake.client().keep_alive(&lease(Duration::from_nanos(2)));
    assert!(matches!(result, Err(Error::TtlTooShort(_))));
}
