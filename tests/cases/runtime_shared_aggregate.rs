//! Deterministic scheduler around the real runtime aggregate transition.
//! Local atomics isolate this test from unrelated runtime workers.
use super::*;
use std::sync::{Arc, mpsc};

fn interleave(a_on: bool) {
    let flag = Arc::new(AtomicU8::new(u8::from(!a_on)));
    let count = Arc::new(AtomicUsize::new(usize::from(!a_on)));
    let a = AtomicBool::new(!a_on);
    let b = Arc::new(AtomicBool::new(false));
    let (go_tx, go_rx) = mpsc::channel();
    let (step_tx, step_rx) = mpsc::channel();
    let thread = {
        let (flag, count, b) = (flag.clone(), count.clone(), b.clone());
        std::thread::spawn(move || {
            go_rx.recv().unwrap();
            // If A owns the transition lock, acknowledge contention before
            // entering the real helper. Otherwise complete B first, forcing
            // A's stale publication. No sleeps, timeouts or chance scheduling.
            let blocked = match SHARED_TRANSITION.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    false
                }
                Err(std::sync::TryLockError::WouldBlock) => true,
                Err(e) => panic!("transition lock poisoned: {e}"),
            };
            if blocked {
                step_tx.send(()).unwrap();
            }
            set_shared(&flag, &count, &b, true);
            if a_on {
                set_shared(&flag, &count, &b, false);
            }
            if !blocked {
                step_tx.send(()).unwrap();
            }
        })
    };
    AFTER_SHARED_COUNT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            go_tx.send(()).unwrap();
            step_rx.recv().unwrap();
        }));
    });
    set_shared(&flag, &count, &a, a_on);
    thread.join().unwrap();
    let expected = usize::from(a.load(Ordering::Acquire)) + usize::from(b.load(Ordering::Acquire));
    assert_eq!(count.load(Ordering::Acquire), expected);
    assert_eq!(
        flag.load(Ordering::Acquire),
        u8::from(expected != 0),
        "aggregate flag must reflect remaining worker contributions"
    );
    // Duplicate transitions must neither underflow nor double-count.
    set_shared(&flag, &count, &a, a_on);
    assert_eq!(count.load(Ordering::Acquire), expected);
    set_shared(&flag, &count, &a, false);
    set_shared(&flag, &count, &b, false);
    assert_eq!(count.load(Ordering::Acquire), 0);
    assert_eq!(flag.load(Ordering::Acquire), 0);
}

#[test]
fn clearing_worker_cannot_hide_new_request() {
    interleave(false);
}

#[test]
fn raising_worker_survives_other_worker_round_trip() {
    interleave(true);
}
