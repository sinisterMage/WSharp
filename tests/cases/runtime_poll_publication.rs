//! Deterministic regression for #32; no sleeps or scheduler assumptions.
use super::*;
use std::cell::RefCell;

type PublicationHook = Box<dyn FnOnce(Phase)>;

thread_local! {
    static AFTER_STORE: RefCell<Option<PublicationHook>> = RefCell::new(None);
}

pub(super) fn after_phase_store(phase: Phase) {
    let hook = AFTER_STORE.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(phase);
    }
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[test]
fn direct_pause_cannot_leave_a_late_mark_request() {
    let _serial = serial();
    set_phase(Phase::Marking);
    // Run the mutator at the exact publication boundary, before the
    // collector's next statement. An empty heap needs no generated roots.
    AFTER_STORE.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(|phase| {
            assert_eq!(phase, Phase::MarkDone);
            unsafe { safepoint() };
            assert_eq!(super::phase(), Phase::Sweeping);
        }))
    });
    mark_concurrently();
    let leaked = me().mark.wanted.load(Ordering::Acquire);
    gc::clear_poll(me());
    set_phase(Phase::Idle);
    assert!(!leaked, "request raised after its mark pause already ran");
    assert!(!crate::worker::poll_wanted());
}

#[test]
fn evacuation_phase_is_published_with_its_request() {
    let _serial = serial();
    set_phase(Phase::Evacuating);
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    let capture = observed.clone();
    AFTER_STORE.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |phase| {
            assert_eq!(phase, Phase::EvacDone);
            capture.set(me().mark.wanted.load(Ordering::Acquire));
        }))
    });
    evacuate_concurrently();
    gc::clear_poll(me());
    set_phase(Phase::Idle);
    assert!(
        observed.get(),
        "evacuation pause published before its request"
    );
    assert!(!crate::worker::poll_wanted());
}

#[test]
fn early_poll_preserves_request_until_phase_is_ready() {
    let _serial = serial();
    for phase in [Phase::Marking, Phase::Evacuating] {
        set_phase(phase);
        gc::request_safepoint(me());
        gc::ws_gc_poll();
        let retained = me().mark.wanted.load(Ordering::Acquire);
        let flagged = crate::worker::poll_wanted();
        gc::clear_poll(me());
        set_phase(Phase::Idle);
        assert!(retained && flagged, "early poll consumed the pending pause");
    }
}
