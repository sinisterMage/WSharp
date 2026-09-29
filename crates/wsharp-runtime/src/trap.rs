//! The two faults a W# program can cause, reported as W# panics.
//!
//! A program the compiler accepts can still do two things that end in a
//! hardware fault rather than in a check: read through a null reference -- an
//! element of `array.new(n)` that was never assigned, whose field, closure or
//! overload is then asked for -- and recurse deeper than its stack. Both used
//! to end the process with a signal and no W# diagnostic, which the release
//! criteria make a P1 whatever the documentation says (#48, #67). Testing for
//! them in generated code would cost a compare and a branch at every field
//! read and a stack-limit load in every prologue, to catch what the hardware
//! already catches for nothing; so, as Go and the JVM do, the fault is allowed
//! to happen and is then recognised.
//!
//! **What is recognised is narrow on purpose.** A fault is the program's only
//! when it happens on a thread that is running W# ([`enter`]) and it is one of:
//!
//! * a stack overflow -- an access in, or just below, that thread's own stack,
//!   which is where its guard page is; or
//! * an access within [`NULL_WINDOW`] of address zero while the thread is *not*
//!   parked in a safe region -- a header, field or closure read through null.
//!
//! Everything else -- a wild pointer, anything on a collector thread, a null
//! read inside a system call or a foreign function -- goes back to whatever
//! handled the fault before, so a runtime bug is still a crash that looks like
//! one rather than a W# panic that blames the program. That is also why the
//! safe-region test applies to null and not to the stack: a guard page reached
//! from inside a system call is still the program's recursion.
//!
//! **What happens then is what [`ws_panic`](crate::builtins::ws_panic) does,
//! minus what a handler may not do.** One unbuffered write of the message, the
//! same kill of child processes, and an exit with
//! [`PANIC_EXIT_STATUS`](crate::builtins::PANIC_EXIT_STATUS) that runs nothing
//! on the way out. Nothing is unwound, as nothing ever is on a W# panic, and
//! standard output needs no flush: `print` writes whole lines through Rust's
//! line-buffered handle, so every finished line has already gone.
//!
//! Generated code keeps its half of the bargain in the code generator: frames
//! larger than a page are probed on the way in (`enable_probestack`), so a
//! runaway recursion cannot step over the guard page into somebody else's
//! memory instead of faulting.

use crate::builtins::PANIC_EXIT_STATUS;
use crate::sys::imp::trap as os;
use std::marker::PhantomData;
use std::sync::Once;
use std::sync::atomic::{AtomicU8, Ordering};

/// How close to address zero a fault must be to be a read through null.
///
/// Every read through a W# reference is at a small constant offset from it --
/// a header word, a field, a closure's code pointer -- and an array's elements
/// are never addressed without a bounds check that reads the length first.
/// Nothing is ever mapped this low: Linux refuses below `mmap_min_addr`, which
/// is this number, and Windows and macOS reserve at least as much.
pub const NULL_WINDOW: usize = 64 * 1024;

/// Which of the two faults it was.
#[derive(Clone, Copy)]
enum Fault {
    Null,
    Stack,
}

impl Fault {
    fn message(self) -> &'static [u8] {
        match self {
            Fault::Null => {
                b"W# panic: read through a null reference: an element of `array.new(n)` \
                  that was never assigned has no fields, no closure and no overload\n"
            }
            Fault::Stack => {
                b"W# panic: stack overflow: calls went deeper than this thread's stack \
                  allows -- is there a recursion with no base case?\n"
            }
        }
    }
}

/// Report `fault` and end the process, from inside a handler.
fn report(fault: Fault) -> ! {
    // As `report_and_exit`, so that a program which panics this way leaves no
    // children behind either. It takes no lock it would wait for.
    crate::process::kill_all();
    os::write_stderr(fault.message());
    os::exit_now(PANIC_EXIT_STATUS)
}

/// Whether a null fault on a thread whose worker's park state is `parked` is
/// the program's: not while it is inside a system call or a foreign function.
fn running_w_sharp(parked: &AtomicU8) -> bool {
    parked.load(Ordering::Acquire) != crate::worker::PARKED
}

/// Start recognising faults, once per process.
///
/// Called by both backends immediately before the program's `main`, and not
/// earlier: a fault while *compiling* is a compiler bug and must stay a crash.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if os::SUPPORTED {
            imp::install();
        }
    });
}

/// This thread is about to run W# code, until the answer is dropped.
///
/// Every thread that enters generated code holds one: the main thread for the
/// whole of `main`, and each worker for its whole life.
pub fn enter() -> Mutator {
    let worker = crate::worker::Worker::current();
    let registration = if os::SUPPORTED {
        imp::enter(&worker.parked)
    } else {
        None
    };
    Mutator {
        _registration: registration,
        _thread: PhantomData,
    }
}

/// A thread registered by [`enter`]. Dropping it unregisters the thread.
pub struct Mutator {
    _registration: Option<imp::Registration>,
    /// Registration is a fact about the thread that made it.
    _thread: PhantomData<*const ()>,
}

#[cfg(unix)]
mod imp {
    use super::{Fault, NULL_WINDOW, os};
    use core::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering::Relaxed};

    #[allow(non_camel_case_types)]
    type c_int = i32;

    /// How many threads may run W# at once and still have their faults
    /// recognised. One past it runs normally, and faults as it always did.
    const SLOTS: usize = 256;

    /// The alternate stack given to a thread that has none. The handler
    /// writes one line and exits, so this is generous.
    const ALT_STACK: usize = 64 * 1024;

    /// How far below the lowest address of a thread's stack a guard-page fault
    /// may land. A frame larger than a page is probed a page at a time, so the
    /// first fault is at the top of the guard region; the region itself is up
    /// to a megabyte on Linux's main thread. Windows needs no such number: it
    /// says "stack overflow" in so many words.
    const BELOW_STACK: usize = 1 << 20;

    /// One registered thread, as the handler needs to see it.
    ///
    /// The handler finds its slot by the address of its own frame, which is on
    /// the thread's alternate stack, rather than through thread-local storage:
    /// on macOS the first touch of a thread-local from a thread allocates, and a
    /// handler that can call `malloc` can deadlock in it. A thread that never
    /// registered therefore finds nothing and is left alone.
    struct Slot {
        alt_low: AtomicUsize,
        alt_high: AtomicUsize,
        stack_low: AtomicUsize,
        stack_high: AtomicUsize,
        parked: AtomicPtr<AtomicU8>,
    }

    impl Slot {
        const fn empty() -> Slot {
            Slot {
                alt_low: AtomicUsize::new(0),
                alt_high: AtomicUsize::new(0),
                stack_low: AtomicUsize::new(0),
                stack_high: AtomicUsize::new(0),
                parked: AtomicPtr::new(null_mut()),
            }
        }
    }

    static TABLE: [Slot; SLOTS] = [const { Slot::empty() }; SLOTS];

    /// What handled each signal before, put back when a fault is not ours.
    /// Written once, by `install`, before the handler can run.
    static mut PREVIOUS_SEGV: [u64; 32] = [0; 32];
    static mut PREVIOUS_BUS: [u64; 32] = [0; 32];

    pub(super) fn install() {
        // A zeroed `struct sigaction` with the handler at the front and the
        // flags where this system keeps them: an empty mask, no restorer.
        let mut action = [0u64; 32];
        let bytes = action.as_mut_ptr() as *mut u8;
        let handler: extern "C" fn(c_int, *mut u8, *mut c_void) = on_fault;
        unsafe {
            (bytes as *mut usize).write(handler as usize);
            (bytes.add(os::SA_FLAGS_OFFSET) as *mut c_int).write(os::SA_SIGINFO | os::SA_ONSTACK);
            os::sigaction(os::SIGSEGV, bytes, (&raw mut PREVIOUS_SEGV) as *mut u8);
            os::sigaction(os::SIGBUS, bytes, (&raw mut PREVIOUS_BUS) as *mut u8);
        }
    }

    /// A claimed slot, and the alternate stack if it is ours to take down.
    pub(in super::super) struct Registration {
        slot: &'static Slot,
        alt_stack: Option<Box<[u8]>>,
    }

    pub(super) fn enter(parked: &'static AtomicU8) -> Option<Registration> {
        // A handler for a stack overflow cannot run on the stack that
        // overflowed. Rust gives every thread it starts an alternate stack once
        // its own handler is installed, which it is under `wsharp run`; a
        // compiled program never runs Rust's `main`, so there it is ours.
        let (alt_low, alt_size, alt_stack) = match current_alt_stack() {
            Some((low, size)) => (low, size, None),
            None => {
                let memory = vec![0u8; ALT_STACK].into_boxed_slice();
                let low = memory.as_ptr() as usize;
                if !set_alt_stack(low, ALT_STACK, false) {
                    return None;
                }
                (low, ALT_STACK, Some(memory))
            }
        };
        let (stack_low, stack_high) = os::stack_bounds().unwrap_or((0, 0));

        for slot in &TABLE {
            if slot
                .alt_low
                .compare_exchange(0, alt_low, Relaxed, Relaxed)
                .is_ok()
            {
                // Only this thread's handler reads these, and a synchronous
                // fault is delivered on the thread that caused it, so program
                // order is the only ordering needed.
                slot.stack_low.store(stack_low, Relaxed);
                slot.stack_high.store(stack_high, Relaxed);
                slot.parked
                    .store(parked as *const AtomicU8 as *mut AtomicU8, Relaxed);
                slot.alt_high.store(alt_low + alt_size, Relaxed);
                return Some(Registration { slot, alt_stack });
            }
        }
        if alt_stack.is_some() {
            set_alt_stack(0, 0, true);
        }
        None
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            let slot = self.slot;
            slot.alt_high.store(0, Relaxed);
            slot.parked.store(null_mut(), Relaxed);
            slot.stack_low.store(0, Relaxed);
            slot.stack_high.store(0, Relaxed);
            // Last, because it is what frees the slot for another thread.
            slot.alt_low.store(0, Relaxed);
            if self.alt_stack.is_some() {
                // Disabled before the memory goes, so no signal can land on it.
                set_alt_stack(0, 0, true);
            }
        }
    }

    /// The alternate stack this thread already has, as `(low, size)`.
    fn current_alt_stack() -> Option<(usize, usize)> {
        let mut old = [0u64; 8];
        let p = old.as_mut_ptr() as *mut u8;
        if unsafe { os::sigaltstack(null(), p) } != 0 {
            return None;
        }
        let (low, flags, size) = unsafe {
            (
                (p as *const usize).read(),
                (p.add(os::SS_FLAGS_OFFSET) as *const c_int).read(),
                (p.add(os::SS_SIZE_OFFSET) as *const usize).read(),
            )
        };
        (flags & os::SS_DISABLE == 0 && low != 0 && size != 0).then_some((low, size))
    }

    fn set_alt_stack(low: usize, size: usize, disable: bool) -> bool {
        let mut stack = [0u64; 8];
        let p = stack.as_mut_ptr() as *mut u8;
        unsafe {
            (p as *mut usize).write(low);
            (p.add(os::SS_SIZE_OFFSET) as *mut usize).write(size);
            (p.add(os::SS_FLAGS_OFFSET) as *mut c_int).write(if disable {
                os::SS_DISABLE
            } else {
                0
            });
            os::sigaltstack(p, null_mut()) == 0
        }
    }

    /// The slot whose alternate stack holds `here`.
    fn find(here: usize) -> Option<&'static Slot> {
        TABLE.iter().find(|slot| {
            let low = slot.alt_low.load(Relaxed);
            low != 0 && here >= low && here < slot.alt_high.load(Relaxed)
        })
    }

    fn classify(slot: &Slot, address: usize) -> Option<Fault> {
        let low = slot.stack_low.load(Relaxed);
        let high = slot.stack_high.load(Relaxed);
        // Anywhere in the thread's own stack counts, not only below it: every
        // page of it is readable and writable except a guard, and on Linux's
        // main thread a fault inside the range is the kernel refusing to grow
        // the stack any further.
        if low != 0 && address < high && address >= low.saturating_sub(BELOW_STACK) {
            return Some(Fault::Stack);
        }
        let parked = slot.parked.load(Relaxed);
        if address < NULL_WINDOW && !parked.is_null() && super::running_w_sharp(unsafe { &*parked })
        {
            return Some(Fault::Null);
        }
        None
    }

    extern "C" fn on_fault(signal: c_int, info: *mut u8, _context: *mut c_void) {
        let marker = 0u8;
        let here = core::hint::black_box(&marker) as *const u8 as usize;
        let address = unsafe { (info.add(os::SI_ADDR_OFFSET) as *const usize).read() };
        if let Some(slot) = find(here)
            && let Some(fault) = classify(slot, address)
        {
            super::report(fault);
        }
        // Not the program's. Put back what was there and return: the faulting
        // instruction runs again and faults under the old disposition -- Rust's
        // own guard-page handler under `wsharp run`, the default elsewhere.
        let previous = if signal == os::SIGBUS {
            (&raw const PREVIOUS_BUS) as *const u8
        } else {
            (&raw const PREVIOUS_SEGV) as *const u8
        };
        unsafe { os::sigaction(signal, previous, null_mut()) };
    }
}

#[cfg(windows)]
mod imp {
    use super::{Fault, NULL_WINDOW, os};
    use std::cell::Cell;
    use std::ptr::null;
    use std::sync::atomic::AtomicU8;

    /// How much stack to keep back for the handler: enough to kill children,
    /// write a line and terminate.
    const RESERVE: u32 = 32 * 1024;

    thread_local! {
        /// This thread's worker's park state, or null if it is not running W#.
        /// Safe to read from a vectored handler, which is ordinary code on the
        /// faulting thread rather than a signal handler.
        static PARKED: Cell<*const AtomicU8> = const { Cell::new(null()) };
    }

    pub(super) fn install() {
        // `1`: ahead of every other vectored handler, Rust's own included.
        unsafe { os::AddVectoredExceptionHandler(1, on_exception) };
    }

    pub(in super::super) struct Registration;

    pub(super) fn enter(parked: &'static AtomicU8) -> Option<Registration> {
        os::reserve_stack(RESERVE);
        PARKED.with(|p| p.set(parked));
        Some(Registration)
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            let _ = PARKED.try_with(|p| p.set(null()));
        }
    }

    unsafe extern "system" fn on_exception(pointers: *mut os::EXCEPTION_POINTERS) -> i32 {
        let parked = PARKED.try_with(|p| p.get()).unwrap_or(null());
        if parked.is_null() {
            return os::EXCEPTION_CONTINUE_SEARCH;
        }
        let record = unsafe { &*(*pointers).ExceptionRecord };
        match record.ExceptionCode {
            os::STATUS_STACK_OVERFLOW => super::report(Fault::Stack),
            os::STATUS_ACCESS_VIOLATION
                if record.NumberParameters >= 2
                    && record.ExceptionInformation[1] < NULL_WINDOW
                    && super::running_w_sharp(unsafe { &*parked }) =>
            {
                super::report(Fault::Null)
            }
            _ => os::EXCEPTION_CONTINUE_SEARCH,
        }
    }
}
