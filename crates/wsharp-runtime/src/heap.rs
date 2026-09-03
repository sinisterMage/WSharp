//! The W# heap: an Immix-style space of blocks and lines.
//!
//! Every allocation the language performs goes through [`ws_alloc`], which is
//! what makes the collector's job tractable: there is one place objects are
//! born, one place their headers are stamped, and one place a collection can be
//! triggered from.
//!
//! The shape follows LXR (Zuo, Blackburn, Zigman & Yang, PLDI 2022), which
//! builds on Immix:
//!
//! * **Blocks** of 32 KiB are the unit of acquisition and of evacuation.
//! * **Lines** of 256 bytes are the unit of reclamation. A line with no live
//!   object on it is reusable without moving anything.
//! * Side metadata -- a mark byte per line, a state per block -- lives outside
//!   the objects, indexed by arithmetic on the address. That is why blocks come
//!   from *over-aligned* reservations: given any pointer, subtracting the
//!   space's base and shifting yields its block and line with no search.
//!
//! Objects too big for a quarter of a block go to a separate large-object
//! space. They are never moved and never share a block, so a 1 MiB array costs
//! one allocation rather than fragmenting the main space.
//!
//! Reclamation is driven from [`crate::gc`]: this module knows how to free an
//! object and recycle a block, but not when to. The previous bump allocator
//! could not have supported either -- it overwrote its cursor on every new
//! chunk, and so forgot how much of the old one was in use.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::sync::Mutex;

use crate::header::{ALIGN, FLAG_LOGGED, HEADER_SIZE, TypeId, align_up, meta_word};

/// 32 KiB. Immix's canonical size, and the granularity spaces are aligned to.
pub const BLOCK_BITS: u32 = 15;
pub const BLOCK_BYTES: usize = 1 << BLOCK_BITS;
/// 256 bytes, giving 128 lines per block.
pub const LINE_BITS: u32 = 8;
pub const LINE_BYTES: usize = 1 << LINE_BITS;
pub const LINES_PER_BLOCK: usize = BLOCK_BYTES / LINE_BYTES;

/// Objects at least this large bypass the block space entirely. A quarter of a
/// block: above that, sharing a block wastes more than it saves.
pub const LARGE_OBJECT_BYTES: usize = BLOCK_BYTES / 4;

/// How much address space one reservation covers. Spaces are added as needed,
/// so this is a growth granularity rather than a limit.
const SPACE_BLOCKS: usize = 2048; // 64 MiB

/// What the collector knows about a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// No live object; available for allocation.
    Free,
    /// Being allocated into.
    Open,
    /// Full, with live objects.
    Full,
    /// Selected for evacuation. The load barrier watches for this.
    Evacuating,
}

struct Block {
    state: BlockState,
    /// Bytes used from the start of the block.
    cursor: u32,
    /// Lines holding at least one live object.
    live_lines: u16,
}

/// The most objects that can share one line: everything is 16-byte aligned and
/// carries a 16-byte header, so a 256-byte line holds at most sixteen.
const MAX_OBJECTS_PER_LINE: usize = LINE_BYTES / ALIGN;

/// One over-aligned reservation, carved into blocks.
struct Space {
    base: usize,
    bytes: usize,
    layout: Layout,
    blocks: Vec<Block>,
    /// Live objects occupying each line, indexed
    /// `block * LINES_PER_BLOCK + line`. A count rather than a mark bit,
    /// because reclamation is per line and a line is only reusable once every
    /// object touching it is gone -- including one that started on the line
    /// before.
    line_objects: Vec<u8>,
}

impl Space {
    fn new(blocks: usize) -> Space {
        let bytes = blocks * BLOCK_BYTES;
        // Aligning the whole reservation to the block size is what makes
        // `(addr - base) >> BLOCK_BITS` a valid block index.
        let layout = Layout::from_size_align(bytes, BLOCK_BYTES).expect("valid heap space layout");
        // Zeroed so a partially initialised object's fields read as null,
        // which keeps it safe to trace at any moment.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Space {
            base: ptr as usize,
            bytes,
            layout,
            blocks: (0..blocks)
                .map(|_| Block {
                    state: BlockState::Free,
                    cursor: 0,
                    live_lines: 0,
                })
                .collect(),
            line_objects: vec![0; blocks * LINES_PER_BLOCK],
        }
    }

    fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base + self.bytes
    }

    fn block_index(&self, addr: usize) -> usize {
        (addr - self.base) >> BLOCK_BITS
    }

    fn line_index(&self, addr: usize) -> usize {
        (addr - self.base) >> LINE_BITS
    }

    fn block_start(&self, block: usize) -> usize {
        self.base + block * BLOCK_BYTES
    }

    /// The lines an object at `addr` of `size` bytes touches.
    fn lines_of(&self, addr: usize, size: usize) -> std::ops::RangeInclusive<usize> {
        self.line_index(addr)..=self.line_index(addr + size - 1)
    }

    /// Record an object as occupying its lines. Returns how many lines went
    /// from empty to occupied.
    fn occupy(&mut self, addr: usize, size: usize) -> u16 {
        let mut newly = 0;
        for line in self.lines_of(addr, size) {
            debug_assert!((self.line_objects[line] as usize) < MAX_OBJECTS_PER_LINE);
            if self.line_objects[line] == 0 {
                newly += 1;
            }
            self.line_objects[line] += 1;
        }
        newly
    }

    /// Release an object's lines. Returns how many lines became empty.
    fn vacate(&mut self, addr: usize, size: usize) -> u16 {
        let mut freed = 0;
        for line in self.lines_of(addr, size) {
            debug_assert!(self.line_objects[line] > 0, "line freed twice");
            self.line_objects[line] -= 1;
            if self.line_objects[line] == 0 {
                freed += 1;
            }
        }
        freed
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        unsafe { dealloc(self.base as *mut u8, self.layout) };
    }
}

struct LargeObject {
    ptr: *mut u8,
    layout: Layout,
}

pub struct Heap {
    spaces: Vec<Space>,
    /// The block currently being bumped into, as `(space, block)`.
    open: Option<(usize, usize)>,
    large: Vec<LargeObject>,
    /// Cumulative, never reduced: what the program has asked for in total.
    bytes_allocated: usize,
    objects_allocated: usize,
    /// Current, reduced by every reclamation: what is still alive.
    live_objects: usize,
    live_bytes: usize,
}

// The heap hands out raw pointers into memory it owns for the lifetime of the
// process; the `Mutex` makes concurrent access to the bump pointer safe.
unsafe impl Send for Heap {}

impl Heap {
    const fn new() -> Heap {
        Heap {
            spaces: Vec::new(),
            open: None,
            large: Vec::new(),
            bytes_allocated: 0,
            objects_allocated: 0,
            live_objects: 0,
            live_bytes: 0,
        }
    }

    /// True if `addr` points into the block space.
    ///
    /// This is the collector's "is this mine?" test, and it replaces checking
    /// an immortal flag on the hot path: string literals and the singleton
    /// instances of zero-field structs live in the JIT's data section, so they
    /// fall outside every space and are excluded structurally.
    fn owns(&self, addr: usize) -> bool {
        self.spaces.iter().any(|s| s.contains(addr))
    }

    /// Find a block with room, opening a new one -- and a new space, if need be.
    fn open_block(&mut self, size: usize) -> (usize, usize) {
        if let Some((s, b)) = self.open
            && self.spaces[s].blocks[b].cursor as usize + size <= BLOCK_BYTES
        {
            return (s, b);
        }
        if let Some((s, b)) = self.open {
            self.spaces[s].blocks[b].state = BlockState::Full;
        }
        for s in 0..self.spaces.len() {
            if let Some(b) = self.spaces[s]
                .blocks
                .iter()
                .position(|blk| blk.state == BlockState::Free)
            {
                self.spaces[s].blocks[b].state = BlockState::Open;
                self.open = Some((s, b));
                return (s, b);
            }
        }
        self.spaces.push(Space::new(SPACE_BLOCKS));
        let s = self.spaces.len() - 1;
        self.spaces[s].blocks[0].state = BlockState::Open;
        self.open = Some((s, 0));
        (s, 0)
    }

    fn alloc_large(&mut self, type_id: TypeId, size: usize) -> *mut u8 {
        self.live_objects += 1;
        self.live_bytes += size;
        let layout = Layout::from_size_align(size, ALIGN).expect("valid large object layout");
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        self.large.push(LargeObject { ptr, layout });
        self.stamp(ptr, type_id);
        ptr
    }

    /// Write the header of a freshly allocated object.
    fn stamp(&mut self, ptr: *mut u8, type_id: TypeId) {
        // Born logged: a new object's fields are all null, so the write barrier
        // has nothing to record about it and can skip its slow path entirely.
        // Born with a count of zero: LXR counts *heap* references, and a new
        // object is so far reachable only from the stack.
        unsafe { (ptr as *mut u64).write(meta_word(type_id, FLAG_LOGGED)) };
    }

    fn alloc(&mut self, type_id: TypeId, size: u32) -> *mut u8 {
        let size = align_up(size.max(HEADER_SIZE)) as usize;
        self.bytes_allocated += size;
        self.objects_allocated += 1;

        if size >= LARGE_OBJECT_BYTES {
            return self.alloc_large(type_id, size);
        }

        let (s, b) = self.open_block(size);
        let space = &mut self.spaces[s];
        let offset = space.blocks[b].cursor as usize;
        let addr = space.block_start(b) + offset;
        space.blocks[b].cursor += size as u32;
        // The collector derives an object's block by shifting its address, so
        // an object that straddled a boundary would be attributed to the wrong
        // one. `open_block` skips the tail of a block rather than splitting.
        debug_assert_eq!(space.block_index(addr), b);
        debug_assert_eq!(space.block_index(addr + size - 1), b);

        // Reclamation works a line at a time, so an object spanning two lines
        // pins both until it dies.
        let newly = space.occupy(addr, size);
        space.blocks[b].live_lines += newly;

        self.live_objects += 1;
        self.live_bytes += size;

        let ptr = addr as *mut u8;
        self.stamp(ptr, type_id);
        ptr
    }

    /// Reclaim one object.
    ///
    /// Its lines go back when nothing else is on them, and a block whose lines
    /// are all empty is recycled whole. Note what this does *not* do: hand back
    /// the free lines of a block that still holds something. A single survivor
    /// pins its whole block, which is the fragmentation evacuation exists to
    /// fix -- and until evacuation lands, the honest cost of not having it.
    fn free(&mut self, ptr: *mut u8, size: usize) {
        let addr = ptr as usize;
        self.live_objects -= 1;
        self.live_bytes -= size;
        // Tombstone rather than erase: a linear walk of the block needs the
        // type id to know how far to step, so the header stays readable and
        // only gains a bit saying the object is gone.
        unsafe { crate::header::set_flag(ptr, crate::header::FLAG_DEAD) };

        if let Some(index) = self.large.iter().position(|o| o.ptr == ptr) {
            let object = self.large.swap_remove(index);
            unsafe { dealloc(object.ptr, object.layout) };
            return;
        }

        let Some(s) = self.spaces.iter().position(|s| s.contains(addr)) else {
            debug_assert!(false, "freeing a pointer the heap does not own");
            return;
        };
        let space = &mut self.spaces[s];
        let b = space.block_index(addr);
        let freed = space.vacate(addr, size);
        space.blocks[b].live_lines -= freed;

        // A block with nothing left in it is reusable from the top. The block
        // currently being bumped into is left alone: its cursor is live.
        if space.blocks[b].live_lines == 0
            && space.blocks[b].state != BlockState::Open
            && self.open != Some((s, b))
        {
            space.blocks[b].state = BlockState::Free;
            space.blocks[b].cursor = 0;
        }
    }

    /// Visit every live object in the heap, in address order.
    ///
    /// Blocks are filled by bumping, so their objects are laid end to end and a
    /// linear scan finds them all -- provided every object's size can be read
    /// back, which is why freeing tombstones a header rather than erasing it.
    ///
    /// This is what the backup trace sweeps over, and reference counting alone
    /// never needs. It is also why the bump allocator had to go: it forgot how
    /// far into each chunk it had got.
    fn for_each_object(&self, mut visit: impl FnMut(*mut u8)) {
        for space in &self.spaces {
            for (b, block) in space.blocks.iter().enumerate() {
                if block.state == BlockState::Free || block.cursor == 0 {
                    continue;
                }
                let start = space.block_start(b);
                let end = start + block.cursor as usize;
                let mut addr = start;
                while addr < end {
                    let ptr = addr as *mut u8;
                    let Some(size) = (unsafe { crate::types::object_size(ptr) }) else {
                        // An unregistered type id means the walk has lost its
                        // place; stopping is safer than guessing a stride.
                        break;
                    };
                    if !unsafe { crate::header::test_flag(ptr, crate::header::FLAG_DEAD) } {
                        visit(ptr);
                    }
                    addr += size.max(ALIGN as u32) as usize;
                }
            }
        }
        for object in &self.large {
            if !unsafe { crate::header::test_flag(object.ptr, crate::header::FLAG_DEAD) } {
                visit(object.ptr);
            }
        }
    }

    /// Mark the sparsest blocks for evacuation, returning how many.
    ///
    /// Freeing works a line at a time, so one survivor in a block pins every
    /// free line around it. Over time a heap fills with mostly-empty blocks it
    /// cannot reuse -- fragmentation that no amount of counting fixes. Moving
    /// the few survivors out of the emptiest blocks is what recovers them, and
    /// it is the reason the header has a forwarding encoding at all.
    ///
    /// The block currently being allocated into is never a candidate: its
    /// cursor is live, and copies have to go somewhere.
    fn select_evacuation(&mut self, max_live_lines: u16) -> usize {
        let open = self.open;
        let mut chosen = 0;
        for (s, space) in self.spaces.iter_mut().enumerate() {
            for (b, block) in space.blocks.iter_mut().enumerate() {
                let sparse = block.live_lines > 0 && block.live_lines <= max_live_lines;
                if sparse && block.state == BlockState::Full && open != Some((s, b)) {
                    block.state = BlockState::Evacuating;
                    chosen += 1;
                }
            }
        }
        chosen
    }

    fn is_evacuating(&self, addr: usize) -> bool {
        self.spaces
            .iter()
            .find(|s| s.contains(addr))
            .is_some_and(|s| s.blocks[s.block_index(addr)].state == BlockState::Evacuating)
    }

    /// Allocate space for a copy, without any of the collector bookkeeping a
    /// program allocation gets: a copy is not a new object, and must not be
    /// enrolled in the nursery or trigger a nested collection.
    fn alloc_copy(&mut self, size: usize) -> *mut u8 {
        if size >= LARGE_OBJECT_BYTES {
            // Large objects are never evacuated, so this cannot be reached.
            debug_assert!(false, "a large object was selected for evacuation");
            return std::ptr::null_mut();
        }
        let (s, b) = self.open_block(size);
        let space = &mut self.spaces[s];
        let addr = space.block_start(b) + space.blocks[b].cursor as usize;
        space.blocks[b].cursor += size as u32;
        let newly = space.occupy(addr, size);
        space.blocks[b].live_lines += newly;
        self.live_objects += 1;
        self.live_bytes += size;
        addr as *mut u8
    }

    /// Account for an object that has gone with its evacuated block, whether
    /// because it was copied out or because it was garbage.
    fn note_evacuated(&mut self, size: usize) {
        self.live_objects -= 1;
        self.live_bytes -= size;
    }

    /// Return every evacuated block to the free list, whole.
    ///
    /// Nothing in them is live any more: the trace visited every reference in
    /// the heap and repointed each one at the copy.
    fn release_evacuated(&mut self) -> usize {
        let mut released = 0;
        for space in &mut self.spaces {
            for (b, block) in space.blocks.iter_mut().enumerate() {
                if block.state != BlockState::Evacuating {
                    continue;
                }
                let base = b * LINES_PER_BLOCK;
                for line in &mut space.line_objects[base..base + LINES_PER_BLOCK] {
                    *line = 0;
                }
                block.live_lines = 0;
                block.cursor = 0;
                block.state = BlockState::Free;
                released += 1;
            }
        }
        released
    }

    pub fn stats(&self) -> HeapStats {
        HeapStats {
            bytes_allocated: self.bytes_allocated,
            objects_allocated: self.objects_allocated,
            live_objects: self.live_objects,
            live_bytes: self.live_bytes,
            blocks: self
                .spaces
                .iter()
                .map(|s| {
                    s.blocks
                        .iter()
                        .filter(|b| b.state != BlockState::Free)
                        .count()
                })
                .sum(),
            large_objects: self.large.len(),
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        for object in self.large.drain(..) {
            unsafe { dealloc(object.ptr, object.layout) };
        }
        // `Space` frees its own reservation.
        self.spaces.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeapStats {
    /// Cumulative totals: what the program has asked for since it started.
    pub bytes_allocated: usize,
    pub objects_allocated: usize,
    /// What is still alive right now. The difference between these and the
    /// totals above is what the collector has reclaimed.
    pub live_objects: usize,
    pub live_bytes: usize,
    /// Blocks handed out; free blocks in a reserved space do not count.
    pub blocks: usize,
    pub large_objects: usize,
}

static HEAP: Mutex<Heap> = Mutex::new(Heap::new());

fn with_heap<R>(f: impl FnOnce(&mut Heap) -> R) -> R {
    f(&mut HEAP.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Allocate a zeroed object of `size` bytes (header included) and stamp its
/// header with `type_id`.
///
/// This is the single allocation entry point for generated code.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `size` must include
/// [`HEADER_SIZE`] and match the registered layout for `type_id`.
pub extern "C" fn ws_alloc(type_id: TypeId, size: u64) -> *mut u8 {
    let ptr = with_heap(|heap| heap.alloc(type_id, size as u32));
    // A call is a safepoint, so this is one of the program points a collection
    // can actually happen at -- which is why it is also where the stack maps
    // are checked under `--gc-stress`. The frame chain above us is intact here.
    unsafe { crate::gc::on_allocation(ptr) };
    ptr
}

/// True if `ptr` points into the collected heap rather than the module's data
/// section. Immortal objects -- string literals, zero-field singletons -- are
/// outside every space, so this is all the test the collector needs.
pub fn in_heap(ptr: *const u8) -> bool {
    with_heap(|heap| heap.owns(ptr as usize))
}

/// Reclaim an object the collector has proved dead.
///
/// # Safety
/// `ptr` must be a live object this heap allocated, `size` its allocated size,
/// and nothing may reference it.
pub unsafe fn free_object(ptr: *mut u8, size: u32) {
    with_heap(|heap| heap.free(ptr, align_up(size.max(HEADER_SIZE)) as usize));
}

pub fn heap_stats() -> HeapStats {
    with_heap(|heap| heap.stats())
}

/// Every live object in the heap with its size, in address order.
///
/// Collected into a `Vec` rather than visited under the heap lock, because the
/// tracer needs to free as it goes and freeing takes the same lock. The sizes
/// come along because an object that is later forwarded no longer has a
/// readable type id -- the forwarding address overwrote it -- so this is the
/// last chance to ask.
pub fn live_objects() -> Vec<(*mut u8, u32)> {
    let mut out = Vec::new();
    with_heap(|heap| {
        heap.for_each_object(|ptr| {
            let size = unsafe { crate::types::object_size(ptr) }.unwrap_or(HEADER_SIZE);
            out.push((ptr, size));
        })
    });
    out
}

/// Choose the sparsest blocks for evacuation. Returns how many were chosen.
pub fn select_evacuation(max_live_lines: u16) -> usize {
    with_heap(|heap| heap.select_evacuation(max_live_lines))
}

/// True if `ptr` lives in a block that is being evacuated.
pub fn is_evacuating(ptr: *const u8) -> bool {
    with_heap(|heap| heap.is_evacuating(ptr as usize))
}

/// Reserve space for a copy of an object being evacuated.
///
/// # Safety
/// Only for the collector, and only during evacuation.
pub unsafe fn alloc_copy(size: u32) -> *mut u8 {
    with_heap(|heap| heap.alloc_copy(align_up(size.max(HEADER_SIZE)) as usize))
}

/// Account for an object leaving with its evacuated block.
pub fn note_evacuated(size: u32) {
    with_heap(|heap| heap.note_evacuated(align_up(size.max(HEADER_SIZE)) as usize));
}

/// Return every evacuated block to the free list. Returns how many.
pub fn release_evacuated() -> usize {
    with_heap(|heap| heap.release_evacuated())
}

/// Release every reservation and start over. Only for tests -- any W# pointer
/// that outlives this call dangles.
#[doc(hidden)]
pub fn reset_heap_for_tests() {
    with_heap(|heap| *heap = Heap::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{FLAG_LOGGED, TYPE_ID_FIRST_USER, test_flag, type_id_of};

    #[test]
    fn allocations_are_aligned_distinct_and_tagged() {
        let a = ws_alloc(TYPE_ID_FIRST_USER, 24);
        let b = ws_alloc(TYPE_ID_FIRST_USER + 1, 24);
        assert!(!a.is_null() && !b.is_null());
        assert_ne!(a, b);
        assert_eq!(a as usize % ALIGN, 0);
        assert_eq!(b as usize % ALIGN, 0);
        // 24 bytes rounds up to 32, so the objects cannot overlap.
        assert!((b as usize) - (a as usize) >= 32);
        unsafe {
            assert_eq!(type_id_of(a), TYPE_ID_FIRST_USER);
            assert_eq!(type_id_of(b), TYPE_ID_FIRST_USER + 1);
        }
    }

    #[test]
    fn fields_start_zeroed() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        unsafe {
            assert_eq!((p.add(16) as *const u64).read(), 0);
            assert_eq!((p.add(24) as *const u64).read(), 0);
        }
    }

    #[test]
    fn new_objects_are_born_logged() {
        // The write barrier's fast path is a test of this bit, so an object
        // whose fields are all still null must never take the slow path.
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        unsafe { assert!(test_flag(p, FLAG_LOGGED)) };
    }

    #[test]
    fn an_object_larger_than_a_block_still_allocates() {
        let big = (BLOCK_BYTES + 4096) as u64;
        let p = ws_alloc(TYPE_ID_FIRST_USER, big);
        assert!(!p.is_null());
        assert_eq!(p as usize % ALIGN, 0);
        // Writing the last byte must not fault.
        unsafe { p.add(big as usize - 1).write(0xAB) };
        unsafe { assert_eq!(p.add(big as usize - 1).read(), 0xAB) };
        // It went to the large-object space, not into a block.
        assert!(!in_heap(p));
    }

    #[test]
    fn a_zero_size_request_still_gets_a_header() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 0);
        let q = ws_alloc(TYPE_ID_FIRST_USER, 0);
        assert!((q as usize) - (p as usize) >= HEADER_SIZE as usize);
    }

    #[test]
    fn block_space_objects_are_recognised_and_others_are_not() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        assert!(in_heap(p), "a normal object lives in a block");

        // A static object, as the code generator emits for a string literal or
        // a zero-field struct's singleton. It must not look like ours.
        #[repr(align(16))]
        // The payload exists to be pointed at, not read.
        #[allow(dead_code)]
        struct Static([u64; 2]);
        let s = Static([meta_word(TYPE_ID_FIRST_USER, 0), 0]);
        assert!(!in_heap(&s as *const Static as *const u8));
    }

    #[test]
    fn allocation_spans_more_than_one_block() {
        // Enough to fill several blocks, proving a new block is opened rather
        // than the cursor running off the end of the first.
        let before = heap_stats();
        let count = (BLOCK_BYTES / 64) * 3;
        let mut last = std::ptr::null_mut();
        for _ in 0..count {
            last = ws_alloc(TYPE_ID_FIRST_USER, 64);
            assert!(!last.is_null());
        }
        let after = heap_stats();
        assert!(after.blocks > before.blocks, "more blocks were opened");
        // `>=` rather than `==`: the heap is process-global and the test
        // binary runs its tests in parallel, so others allocate too.
        assert!(after.objects_allocated - before.objects_allocated >= count);
        // The last object is still a valid, tagged, in-heap object.
        assert!(in_heap(last));
        unsafe { assert_eq!(type_id_of(last), TYPE_ID_FIRST_USER) };
    }

    #[test]
    fn an_object_never_straddles_a_block_boundary() {
        // A block's remaining space is skipped rather than split, so every
        // object is wholly inside one block -- which is what lets the collector
        // derive an object's block by masking its address.
        let size = 1024u64;
        for _ in 0..(BLOCK_BYTES / size as usize) * 2 + 3 {
            let p = ws_alloc(TYPE_ID_FIRST_USER, size) as usize;
            let start_block = p >> BLOCK_BITS;
            let end_block = (p + size as usize - 1) >> BLOCK_BITS;
            assert_eq!(start_block, end_block, "object straddled a block");
        }
    }
}
