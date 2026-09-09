//! Finding the collector's roots on the mutator's stack.
//!
//! A moving collector must know every stack slot holding a heap reference, and
//! must know it *exactly*: a missed root is a use-after-free, and a false root
//! is a corrupted pointer. Cranelift will tell us, but only if we ask, and only
//! about values we declared -- see `declare_value_needs_stack_map` in the code
//! generator.
//!
//! Cranelift emits, per function, a table of `(return address, frame size,
//! offsets)`. Two facts make it usable:
//!
//! * The key is the **return address** of a call, so a stack walk that reads
//!   return addresses can look each frame up directly.
//! * The frame size Cranelift records is `sp_to_fp` -- the distance from the
//!   stack pointer at the safepoint up to the frame pointer. So given a frame's
//!   frame pointer, `sp = fp - frame_size`, and each root is at `sp + offset`.
//!
//! A walk therefore has two halves, and they are not the same problem.
//!
//! *Reaching* generated code means crossing the handful of Rust frames between
//! the collector and the runtime function generated code called into. That is
//! `innermost_generated_frame`, and it is the half that is per-platform: the
//! SysV arms follow `rbp`, which is why the workspace builds with
//! `-Cforce-frame-pointers=yes` (see `.cargo/config.toml`), while Windows has to
//! use the unwind tables because a Win64 prologue is free to establish `rbp` as
//! `lea rbp, [rsp + n]` and nothing marks the outermost frame. See
//! `sys::windows::Frames`.
//!
//! *Walking* generated code is then `rbp` on every platform, because Cranelift's
//! prologue really is `push rbp; mov rbp, rsp` -- it ignores the calling
//! convention -- and the code generator asks for it with
//! `preserve_frame_pointers`. So only the first half has arms, and the walk that
//! reads stack maps has none.
//!
//! The split is also what lets a *parked* worker be walked at all. Crossing the
//! Rust frames needs the parked thread's own frame pointers or its own
//! registers, and neither can be read from outside it, so a worker answers the
//! first half for itself before it parks and publishes the generated frame it
//! found. What the collector then walks is the second half, which is thread
//! independent.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
    "the W# collector walks the stack by following frame pointers, which is \
     implemented for x86-64 and aarch64 only"
);

/// One safepoint within a compiled function.
#[derive(Debug)]
pub struct SafePoint {
    /// Offset of the call's return address from the function's first byte.
    pub return_offset: u32,
    /// Distance from this frame's frame pointer down to its stack pointer.
    pub frame_size: u32,
    /// Byte offsets from the stack pointer of each live heap reference.
    pub roots: Box<[u32]>,
}

/// One compiled function and everywhere in it a collection can happen.
#[derive(Debug)]
pub struct FunctionCode {
    pub base: usize,
    pub len: usize,
    /// Sorted by `return_offset`.
    pub safepoints: Box<[SafePoint]>,
}

impl FunctionCode {
    fn contains(&self, pc: usize) -> bool {
        pc >= self.base && pc < self.base + self.len
    }

    fn safepoint_at(&self, pc: usize) -> Option<&SafePoint> {
        let offset = (pc - self.base) as u32;
        let index = self
            .safepoints
            .binary_search_by_key(&offset, |s| s.return_offset)
            .ok()?;
        Some(&self.safepoints[index])
    }
}

/// Registered functions, sorted by base address. Leaked for the same reason the
/// type table is: generated code outlives any owner we could give it.
static CODE: AtomicPtr<Vec<FunctionCode>> = AtomicPtr::new(ptr::null_mut());

/// How many functions and safepoints are registered, for diagnostics. A run
/// that reports no safepoints is one where nothing is rooted, which would make
/// every root check pass for the wrong reason.
pub fn registered() -> (usize, usize) {
    match code() {
        Some(table) => (table.len(), table.iter().map(|f| f.safepoints.len()).sum()),
        None => (0, 0),
    }
}

/// Publish the stack maps for a whole compiled module.
pub fn register_code(mut funcs: Vec<FunctionCode>) {
    funcs.sort_by_key(|f| f.base);
    for f in &mut funcs {
        // `find` binary-searches these.
        debug_assert!(
            f.safepoints
                .windows(2)
                .all(|w| w[0].return_offset < w[1].return_offset),
            "Cranelift emits stack maps in ascending return-address order"
        );
    }
    CODE.store(Box::into_raw(Box::new(funcs)), Ordering::Release);
}

fn code() -> Option<&'static Vec<FunctionCode>> {
    let table = CODE.load(Ordering::Acquire);
    if table.is_null() {
        return None;
    }
    // Sound: the table is leaked and never mutated after publication.
    Some(unsafe { &*table })
}

/// The compiled function containing `pc`, if it is generated code at all.
pub fn function_at(pc: usize) -> Option<&'static FunctionCode> {
    let table = code()?;
    // `partition_point` gives the first function starting after `pc`; the one
    // before it is the only candidate.
    let index = table.partition_point(|f| f.base <= pc);
    let candidate = table.get(index.checked_sub(1)?)?;
    candidate.contains(pc).then_some(candidate)
}

/// The frame pointer of the function this is inlined into.
///
/// `#[inline(always)]` is the whole point, and it is the opposite of what a
/// helper usually wants: the caller needs *its own* frame, which stays live
/// while it does something else, and not a helper's, which is gone the moment
/// the helper returns. Reading a returned frame pointer after the fact is
/// reading stack the next call is about to reuse.
///
/// Every user must therefore be `#[inline(never)]` itself, or the frame it
/// records is its caller's.
///
/// Not compiled for Windows, and that absence is the point rather than an
/// oversight: reading `rbp` there answers a question about the *current*
/// function and says nothing about its caller, so the one thing this is for --
/// starting a walk -- is exactly what it cannot do. That arm crosses the Rust
/// frames with the unwind tables instead.
#[cfg(not(target_os = "windows"))]
#[inline(always)]
pub(crate) fn current_frame_pointer() -> usize {
    let fp: usize;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack, preserves_flags))
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags))
    };
    fp
}

/// A backstop against an unreadable frame chain. Far above any real W# stack.
const MAX_FRAMES: usize = 1 << 16;

/// A generated frame: its frame pointer, and the program counter within it.
///
/// Both, because a frame pointer alone does not say whose stack map to read.
pub type GeneratedFrame = (usize, usize);

/// Call `visit` with the address of every stack slot holding a live heap
/// reference, for every generated frame below the caller.
///
/// The slot address is handed over rather than the value, so a moving collector
/// can update it in place.
///
/// # Safety
/// Must be called from a runtime function that generated code called into --
/// `ws_alloc` or the collector's poll -- with the frame chain intact.
#[inline(never)]
pub unsafe fn walk_roots(visit: impl FnMut(*mut *mut u8)) {
    if let Some(frame) = innermost_generated_frame() {
        unsafe { walk_generated(frame, visit) };
    }
}

/// The nearest generated frame above the caller, if there is one.
///
/// **Answerable only about the calling thread.** Crossing the Rust frames needs
/// this thread's frame pointers or this thread's registers, and neither can be
/// read from outside, which is why a worker about to park calls this for itself
/// rather than leaving it to whoever walks it afterwards.
///
/// `None` means there is no generated code below the caller at all -- a runtime
/// thread, or a mutator that has not entered W# yet. It is not an error, and it
/// is the answer the old walk could not give: it kept climbing instead, and on
/// Windows climbed straight off the end of the stack.
#[cfg(not(target_os = "windows"))]
#[inline(never)]
pub fn innermost_generated_frame() -> Option<GeneratedFrame> {
    let trace = crate::gc::env_flag("WSHARP_GC_TRACE");
    // This frame's own pointer. Every frame the search wants is above it.
    let mut fp = current_frame_pointer();
    for _ in 0..MAX_FRAMES {
        // `[fp]` is the caller's frame pointer and `[fp + 8]` the return
        // address into it, so each iteration describes the frame *above*.
        let parent_fp = unsafe { (fp as *const usize).read() };
        if parent_fp <= fp {
            // Stacks grow down, so a parent frame is always at a higher
            // address. Anything else means the chain is broken -- and on the
            // outermost frame the ABI's zero makes it so.
            return None;
        }
        let pc = unsafe { ((fp + 8) as *const usize).read() };
        if trace {
            eprintln!("  crossing fp={fp:#x} pc={pc:#x}");
        }
        if function_at(pc).is_some() {
            return Some((parent_fp, pc));
        }
        fp = parent_fp;
    }
    None
}

/// As above, crossing the Rust frames with the unwind tables.
///
/// Windows records a function's frame register in its unwind info rather than
/// promising `push rbp; mov rbp, rsp`, so `rbp` is not a chain here; see
/// [`crate::sys::Frames`] for what that looked like when it was followed anyway.
///
/// **The walk happens inside the capture rather than after it**, because the
/// captured `CONTEXT` describes the frame it was taken in and unwinding out of
/// that frame reads it. See [`crate::sys::Frames::with_here`], which is where
/// that cost a collector that found no roots at all.
///
/// The flag is read before the capture, so that reading it cannot be one of the
/// calls that would have scribbled on the frame being described.
#[cfg(target_os = "windows")]
#[inline(never)]
pub fn innermost_generated_frame() -> Option<GeneratedFrame> {
    let trace = crate::gc::env_flag("WSHARP_GC_TRACE");
    crate::sys::Frames::with_here(|frames| {
        for _ in 0..MAX_FRAMES {
            let pc = frames.pc();
            if trace {
                eprintln!("  crossing pc={pc:#x}");
            }
            if function_at(pc).is_some() {
                return Some((frames.frame_pointer(), pc));
            }
            if !frames.step() {
                return None;
            }
        }
        None
    })
}

/// Visit the roots of `frame` and of every generated frame above it.
///
/// This half needs no per-platform arm. Cranelift's prologue is `push rbp; mov
/// rbp, rsp` whatever the calling convention, so within generated code `[fp]`
/// really is the caller's frame pointer -- and generated frames are contiguous,
/// so the first frame that is not generated code is where W# ends.
///
/// Nothing outside a confirmed generated frame is ever dereferenced, which is
/// what keeps a broken chain from becoming a wild read.
///
/// # Safety
/// `frame` must name a live generated frame on a stack that is not running:
/// this thread's own, or a parked worker's, held parked for the whole walk.
pub unsafe fn walk_generated(frame: GeneratedFrame, mut visit: impl FnMut(*mut *mut u8)) {
    let (mut fp, mut pc) = frame;
    // Read once: this runs per frame, per collection.
    let trace = crate::gc::env_flag("WSHARP_GC_TRACE");

    for _ in 0..MAX_FRAMES {
        let Some(func) = function_at(pc) else {
            return;
        };
        if trace {
            eprintln!(
                "  fp={fp:#x} pc={pc:#x} generated, offset {:#x}, {} roots here",
                pc - func.base,
                func.safepoint_at(pc).map_or(0, |s| s.roots.len()),
            );
        }
        if let Some(safepoint) = func.safepoint_at(pc) {
            let sp = fp - safepoint.frame_size as usize;
            for &offset in safepoint.roots.iter() {
                visit((sp + offset as usize) as *mut *mut u8);
            }
        }
        // A generated frame with no stack map at this pc simply has no live
        // references there; keep walking either way.
        let parent_fp = unsafe { (fp as *const usize).read() };
        if parent_fp <= fp {
            return;
        }
        pc = unsafe { ((fp + 8) as *const usize).read() };
        fp = parent_fp;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SERIAL;

    fn func(base: usize, len: usize, points: Vec<(u32, u32, Vec<u32>)>) -> FunctionCode {
        FunctionCode {
            base,
            len,
            safepoints: points
                .into_iter()
                .map(|(return_offset, frame_size, roots)| SafePoint {
                    return_offset,
                    frame_size,
                    roots: roots.into_boxed_slice(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_program_counter_finds_its_function_and_safepoint() {
        // `CODE` is process-wide, and publishing it is a store every other test
        // that walks a stack reads. The test binary runs in parallel.
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        register_code(vec![
            func(0x3000, 0x100, vec![(0x20, 48, vec![0, 8])]),
            func(
                0x1000,
                0x200,
                vec![(0x10, 32, vec![16]), (0x40, 32, vec![])],
            ),
        ]);

        // Registration sorts by base, so the search works either way round.
        let f = function_at(0x1010).expect("inside the first function");
        assert_eq!(f.base, 0x1000);
        let sp = f
            .safepoint_at(0x1010)
            .expect("a safepoint is recorded here");
        assert_eq!(sp.frame_size, 32);
        assert_eq!(&*sp.roots, &[16]);

        // A safepoint with no live references is still a safepoint.
        assert!(f.safepoint_at(0x1040).expect("recorded").roots.is_empty());
        // A pc inside the function but not at a safepoint has no entry.
        assert!(f.safepoint_at(0x1030).is_none());

        assert_eq!(function_at(0x3020).map(|f| f.base), Some(0x3000));
        // Gaps between functions, and addresses below or above them all, are
        // not generated code -- the walk must not attribute them to a
        // neighbour.
        assert!(function_at(0x1200).is_none());
        assert!(function_at(0x2fff).is_none());
        assert!(function_at(0x0fff).is_none());
        assert!(function_at(0x3100).is_none());
    }

    #[test]
    fn walking_a_stack_with_no_generated_frames_finds_nothing() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        register_code(Vec::new());
        let mut found = 0;
        unsafe { walk_roots(|_| found += 1) };
        assert_eq!(found, 0);
    }

    /// Not run on Windows, where it is false and allowed to be: a Win64
    /// prologue establishes its frame register however its unwind info says,
    /// which is the whole reason that arm does not follow this chain.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn the_frame_pointer_chain_is_readable() {
        // If this fails, `-Cforce-frame-pointers=yes` is not in effect and the
        // walk above would be reading garbage.
        let fp = current_frame_pointer();
        assert_ne!(fp, 0);
        let parent = unsafe { (fp as *const usize).read() };
        assert!(parent > fp, "a parent frame sits at a higher address");
        let pc = unsafe { ((fp + 8) as *const usize).read() };
        assert_ne!(pc, 0, "the return address is present");
    }

    /// A stack with no generated code on it is answered, not climbed.
    ///
    /// This is what a parked worker records, and the answer a runtime thread
    /// gives. It used to have no way to say so: the search ran to the end of
    /// the stack instead, which is harmless where the ABI zeroes the outermost
    /// frame pointer and, on Windows, was an access violation reading whatever
    /// the thread entry had left in the register.
    #[test]
    fn a_stack_with_no_generated_code_reports_none() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        register_code(Vec::new());
        assert!(innermost_generated_frame().is_none());
    }

    /// Walking generated frames stops at the first frame that is not one,
    /// without reading past it.
    ///
    /// The frame handed over is a fabricated one whose pc belongs to no
    /// registered function, so the walk must return before dereferencing it --
    /// which is the property that keeps a broken chain from becoming a wild
    /// read.
    #[test]
    fn walking_stops_before_reading_a_frame_that_is_not_generated() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        register_code(vec![func(0x1000, 0x200, vec![(0x10, 32, vec![16])])]);
        // An address that is not mapped: reaching the read would fault.
        unsafe { walk_generated((0x8, 0x9999), |_| unreachable!()) };
    }
}
