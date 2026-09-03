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
//! Walking needs frame pointers, which is why the code generator sets
//! `preserve_frame_pointers` and why the workspace builds with
//! `-Cforce-frame-pointers=yes` (see `.cargo/config.toml`): the chain has to be
//! unbroken through the Rust runtime frames as well as the generated ones.

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

/// This function's own frame pointer.
///
/// `#[inline(never)]` matters: inlined into a caller, the prologue this reads
/// would be the caller's.
#[inline(never)]
fn frame_pointer() -> usize {
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

/// Call `visit` with the address of every stack slot holding a live heap
/// reference, for every generated frame below the caller.
///
/// The slot address is handed over rather than the value, so a moving collector
/// can update it in place.
///
/// # Safety
/// Must be called from a runtime function that generated code called into --
/// `ws_alloc` or the collector's poll -- with the frame chain intact.
pub unsafe fn walk_roots(mut visit: impl FnMut(*mut *mut u8)) {
    let mut fp = frame_pointer();
    // Generated frames are contiguous: once the walk has entered them, the
    // first frame that is not generated code is where W# ends. That is a
    // sounder stop condition than waiting for a null frame pointer, which
    // depends on whatever libc did below `main`.
    let mut inside = false;
    // Read once: this runs per frame, per collection.
    let trace = std::env::var_os("WSHARP_GC_TRACE").is_some();

    for _ in 0..MAX_FRAMES {
        // `[fp]` is the caller's frame pointer and `[fp + 8]` the return
        // address into it, so each iteration describes the frame *above*.
        let parent_fp = unsafe { (fp as *const usize).read() };
        if parent_fp <= fp {
            // Stacks grow down, so a parent frame is always at a higher
            // address. Anything else means the chain is broken.
            break;
        }
        let pc = unsafe { ((fp + 8) as *const usize).read() };

        if trace {
            match function_at(pc) {
                Some(f) => eprintln!(
                    "  fp={fp:#x} pc={pc:#x} generated, offset {:#x}, {} roots here",
                    pc - f.base,
                    f.safepoint_at(pc).map_or(0, |s| s.roots.len()),
                ),
                None => eprintln!("  fp={fp:#x} pc={pc:#x} not generated"),
            }
        }
        match function_at(pc) {
            Some(func) => {
                inside = true;
                if let Some(safepoint) = func.safepoint_at(pc) {
                    let sp = parent_fp - safepoint.frame_size as usize;
                    for &offset in safepoint.roots.iter() {
                        visit((sp + offset as usize) as *mut *mut u8);
                    }
                }
                // A generated frame with no stack map at this pc simply has no
                // live references there; keep walking.
            }
            None if inside => break,
            None => {}
        }
        fp = parent_fp;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        register_code(Vec::new());
        let mut found = 0;
        unsafe { walk_roots(|_| found += 1) };
        assert_eq!(found, 0);
    }

    #[test]
    fn the_frame_pointer_chain_is_readable() {
        // If this fails, `-Cforce-frame-pointers=yes` is not in effect and the
        // walk above would be reading garbage.
        let fp = frame_pointer();
        assert_ne!(fp, 0);
        let parent = unsafe { (fp as *const usize).read() };
        assert!(parent > fp, "a parent frame sits at a higher address");
        let pc = unsafe { ((fp + 8) as *const usize).read() };
        assert_ne!(pc, 0, "the return address is present");
    }
}
