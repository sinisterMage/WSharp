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
//! * **Lines** of 256 bytes are the unit of reclamation. A run of lines with no
//!   live object on it is a *hole*, and allocation refills holes in blocks that
//!   are still holding something -- one survivor no longer pins the space
//!   around it until a trace comes to evacuate it.
//! * Side metadata -- a count per line, a state per block, a bit per object --
//!   lives outside the objects, indexed by arithmetic on the address. That is
//!   why blocks come from *over-aligned* reservations: given any pointer,
//!   subtracting the space's base and shifting yields its block and line with
//!   no search.
//!
//! Objects too big for a quarter of a block go to a separate large-object
//! space. They are never moved and never share a block, so a 1 MiB array costs
//! one allocation rather than fragmenting the main space.
//!
//! **The object-start bitmap is what makes the heap walkable.** One bit per
//! 16-byte granule says "an object begins here". Enumerating live objects is
//! then a scan of set bits rather than a walk from the start of a block
//! stepping by each object's size -- which matters because neither of the two
//! things above leaves objects laid end to end: a refilled hole puts new
//! objects among the corpses of old ones, and an allocation buffer leaves an
//! unused tail. A stride walk would lose its place at the first such gap, and
//! losing its place means missing live objects, which for the evacuation
//! fix-up means a dangling pointer.
//!
//! **What needs the heap lock, and what does not.** Allocation out of a
//! thread's own buffer takes no lock at all: the bitmap, the line counts and
//! the statistics are atomics, and the buffer's block is marked `Open`, which
//! is a state nothing else allocates into, recycles or evacuates. The lock is
//! for handing out a new buffer, freeing, sweeping and evacuating.
//!
//! Reclamation policy lives in [`crate::gc`] and [`crate::mark`]: this module
//! knows how to free an object, sweep a block and recycle it, but not when to.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU16, AtomicU64, AtomicUsize, Ordering};

use crate::header::{
    ALIGN, FLAG_DEAD, FLAG_IMMORTAL, FLAG_LOGGED, HEADER_SIZE, TypeId, align_up, mark_parity,
    meta_word, set_flag, store_meta, test_flag,
};

/// 32 KiB. Immix's canonical size, and the granularity spaces are aligned to.
pub const BLOCK_BITS: u32 = 15;
pub const BLOCK_BYTES: usize = 1 << BLOCK_BITS;
/// 256 bytes, giving 128 lines per block.
pub const LINE_BITS: u32 = 8;
pub const LINE_BYTES: usize = 1 << LINE_BITS;
pub const LINES_PER_BLOCK: usize = BLOCK_BYTES / LINE_BYTES;

/// One bit of the object-start bitmap covers this many bytes -- the heap's
/// alignment, so every object start lands on exactly one bit.
const GRANULES_PER_BLOCK: usize = BLOCK_BYTES / ALIGN;
const BITMAP_WORDS_PER_BLOCK: usize = GRANULES_PER_BLOCK / 64;

/// Objects at least this large bypass the block space entirely. A quarter of a
/// block: above that, sharing a block wastes more than it saves.
pub const LARGE_OBJECT_BYTES: usize = BLOCK_BYTES / 4;

/// How much address space one reservation covers. Spaces are added as needed,
/// so this is a growth granularity rather than a limit.
const SPACE_BLOCKS: usize = 2048; // 64 MiB
const SPACE_BYTES: usize = SPACE_BLOCKS * BLOCK_BYTES;

/// The most spaces the lock-free directory can name: 16 GiB of block space.
const MAX_SPACES: usize = 256;

/// What the collector knows about a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockState {
    /// No live object; available in its entirety.
    Free = 0,
    /// Owned by an allocator -- the heap's own bump position, or some thread's
    /// buffer. Nothing else allocates into it, and it is never recycled or
    /// evacuated while it is here, which is what lets a buffer's unused tail
    /// sit there unclaimed.
    Open = 1,
    /// Holding live objects, with no hole worth refilling.
    Full = 2,
    /// Holding live objects, with at least one free line: a hole to refill.
    Recyclable = 3,
    /// Selected for evacuation by the trace in progress.
    Evacuating = 4,
}

impl BlockState {
    fn from_u8(v: u8) -> BlockState {
        match v {
            0 => BlockState::Free,
            1 => BlockState::Open,
            2 => BlockState::Full,
            3 => BlockState::Recyclable,
            _ => BlockState::Evacuating,
        }
    }
}

/// The statistics, kept outside the heap so that a thread allocating from its
/// own buffer can maintain them without taking the lock. One set per worker,
/// leaked so that its heap can hold a `&'static` to it.
#[derive(Debug)]
pub(crate) struct Counters {
    /// Cumulative, never reduced: what the program has asked for in total.
    bytes_allocated: AtomicUsize,
    objects_allocated: AtomicUsize,
    /// Current, reduced by every reclamation: what is still alive.
    live_objects: AtomicUsize,
    live_bytes: AtomicUsize,
}

impl Counters {
    pub(crate) const fn new() -> Counters {
        Counters {
            bytes_allocated: AtomicUsize::new(0),
            objects_allocated: AtomicUsize::new(0),
            live_objects: AtomicUsize::new(0),
            live_bytes: AtomicUsize::new(0),
        }
    }

    fn note_allocated(&self, size: usize) {
        self.note_allocated_many(1, size);
    }

    fn note_allocated_many(&self, objects: usize, bytes: usize) {
        self.bytes_allocated.fetch_add(bytes, Ordering::Relaxed);
        self.objects_allocated.fetch_add(objects, Ordering::Relaxed);
        self.live_objects.fetch_add(objects, Ordering::Relaxed);
        self.live_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn note_freed(&self, size: usize) {
        self.live_objects.fetch_sub(1, Ordering::Relaxed);
        self.live_bytes.fetch_sub(size, Ordering::Relaxed);
    }
}

/// A space's side metadata.
///
/// Boxed, and never moved or freed for as long as its space exists, so that a
/// thread's allocation buffer and the lock-free directory can both hold a
/// pointer to it while the `Vec<Space>` that owns the space grows.
#[derive(Debug)]
struct SpaceMeta {
    base: usize,
    /// One state per block. Read without the heap lock by the marker and by
    /// every allocating thread.
    states: Box<[AtomicU8]>,
    /// Live objects occupying each line, indexed
    /// `block * LINES_PER_BLOCK + line`. A count rather than a mark bit,
    /// because reclamation is per line and a line is only reusable once every
    /// object touching it is gone -- including one that started on the line
    /// before.
    line_objects: Box<[AtomicU8]>,
    /// Lines holding at least one live object, per block.
    live_lines: Box<[AtomicU16]>,
    /// One bit per 16-byte granule: "an object begins here". See the module
    /// documentation -- this is what makes the heap walkable when objects are
    /// not laid end to end.
    starts: Box<[AtomicU64]>,
}

impl SpaceMeta {
    fn contains(&self, addr: usize) -> bool {
        addr.wrapping_sub(self.base) < SPACE_BYTES
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

    fn state(&self, block: usize) -> BlockState {
        BlockState::from_u8(self.states[block].load(Ordering::Acquire))
    }

    fn set_state(&self, block: usize, state: BlockState) {
        self.states[block].store(state as u8, Ordering::Release);
    }

    fn live_lines(&self, block: usize) -> u16 {
        self.live_lines[block].load(Ordering::Relaxed)
    }

    // ---- the object-start bitmap ----------------------------------------

    fn granule(&self, addr: usize) -> usize {
        (addr - self.base) / ALIGN
    }

    /// Record that an object begins at `addr`.
    ///
    /// Released, and always *after* the object's header has been stamped, so a
    /// walker that sees the bit sees a readable header behind it.
    fn set_start(&self, addr: usize) {
        let g = self.granule(addr);
        self.starts[g / 64].fetch_or(1u64 << (g % 64), Ordering::Release);
    }

    fn clear_start(&self, addr: usize) {
        let g = self.granule(addr);
        self.starts[g / 64].fetch_and(!(1u64 << (g % 64)), Ordering::Release);
    }

    /// Forget every object in a block. For a block being recycled whole, whose
    /// contents are all dead by construction.
    fn clear_block_starts(&self, block: usize) {
        let base = block * BITMAP_WORDS_PER_BLOCK;
        for word in &self.starts[base..base + BITMAP_WORDS_PER_BLOCK] {
            word.store(0, Ordering::Release);
        }
    }

    /// Every object in `block`, in address order, with its size.
    ///
    /// A scan of the bitmap rather than a walk by stride: see the module
    /// documentation for why the difference matters.
    fn for_each_in_block(&self, block: usize, mut visit: impl FnMut(*mut u8, u32)) {
        let base_word = block * BITMAP_WORDS_PER_BLOCK;
        let base_granule = block * GRANULES_PER_BLOCK;
        for w in 0..BITMAP_WORDS_PER_BLOCK {
            let mut word = self.starts[base_word + w].load(Ordering::Acquire);
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                word &= word - 1;
                let addr = self.base + (base_granule + w * 64 + bit) * ALIGN;
                let ptr = addr as *mut u8;
                // A forwarded object has no readable type id; the caller that
                // forwards is the one that took its size beforehand.
                let size = unsafe { crate::types::object_size(ptr) }
                    .map_or(HEADER_SIZE, |s| align_up(s.max(HEADER_SIZE)));
                visit(ptr, size);
            }
        }
    }

    // ---- lines ----------------------------------------------------------

    /// The lines an object at `addr` of `size` bytes touches.
    fn lines_of(&self, addr: usize, size: usize) -> std::ops::RangeInclusive<usize> {
        self.line_index(addr)..=self.line_index(addr + size - 1)
    }

    /// Record an object as occupying its lines, and its block as holding them.
    fn occupy(&self, addr: usize, size: usize) {
        let block = self.block_index(addr);
        let mut newly = 0u16;
        for line in self.lines_of(addr, size) {
            if self.line_objects[line].fetch_add(1, Ordering::AcqRel) == 0 {
                newly += 1;
            }
        }
        if newly > 0 {
            self.live_lines[block].fetch_add(newly, Ordering::AcqRel);
        }
    }

    /// Release an object's lines. Returns how many became empty.
    fn vacate(&self, addr: usize, size: usize) -> u16 {
        let block = self.block_index(addr);
        let mut freed = 0u16;
        for line in self.lines_of(addr, size) {
            let before = self.line_objects[line].fetch_sub(1, Ordering::AcqRel);
            debug_assert!(before > 0, "line freed twice");
            if before == 1 {
                freed += 1;
            }
        }
        if freed > 0 {
            self.live_lines[block].fetch_sub(freed, Ordering::AcqRel);
        }
        freed
    }

    /// The first hole in `block` at or after `from_line`, as a byte range, big
    /// enough to hold `want` bytes. A hole is a run of lines no live object
    /// touches.
    fn find_hole(&self, block: usize, from_line: usize, want: usize) -> Option<(usize, usize)> {
        let base = block * LINES_PER_BLOCK;
        let mut line = from_line;
        while line < LINES_PER_BLOCK {
            if self.line_objects[base + line].load(Ordering::Acquire) != 0 {
                line += 1;
                continue;
            }
            let start = line;
            while line < LINES_PER_BLOCK
                && self.line_objects[base + line].load(Ordering::Acquire) == 0
            {
                line += 1;
            }
            if (line - start) * LINE_BYTES >= want {
                let addr = self.block_start(block) + start * LINE_BYTES;
                return Some((addr, self.block_start(block) + line * LINE_BYTES));
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// The lock-free directory
// ---------------------------------------------------------------------------

/// The block spaces the process-wide heap has reserved, readable without the
/// heap lock. Spaces are only ever added, and never freed, so an entry, once
/// published, is good for the life of the process. The count is stored last,
/// with release ordering, which is what makes an entry complete when seen.
static PUBLISHED: [AtomicPtr<SpaceMeta>; MAX_SPACES] =
    [const { AtomicPtr::new(ptr::null_mut()) }; MAX_SPACES];
static PUBLISHED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The published space holding `addr`.
fn lookup(addr: usize) -> Option<&'static SpaceMeta> {
    let count = PUBLISHED_COUNT.load(Ordering::Acquire);
    for entry in &PUBLISHED[..count] {
        // Sound: published metadata is boxed, never moved, and never freed.
        let meta = unsafe { &*entry.load(Ordering::Relaxed) };
        if meta.contains(addr) {
            return Some(meta);
        }
    }
    None
}

/// True if `ptr` points into a block space.
///
/// This is an *address* test, for finding an object's block and lines. It is
/// not the test for whether the collector owns an object: large objects live
/// outside every space and are collected all the same. For that, see
/// [`is_collectable`].
pub fn in_heap(ptr: *const u8) -> bool {
    lookup(ptr as usize).is_some()
}

/// True if `ptr` lives in a block the trace in progress is evacuating.
pub fn is_evacuating(ptr: *const u8) -> bool {
    lookup(ptr as usize)
        .is_some_and(|meta| meta.state(meta.block_index(ptr as usize)) == BlockState::Evacuating)
}

/// True if `ptr` is a reference the collector counts, marks and may free.
///
/// The objects it must not touch are the immortal ones: string literals and
/// the singleton instances of zero-field structs, which the code generator
/// emits into the JIT's data section. That section is mapped read-only once
/// the module is finalised, so even an atomic count adjustment on one of them
/// is a fault. Every such object carries `FLAG_IMMORTAL`, which is why this is
/// a header test and not an address-range test: a large object is outside
/// every block space and is collectable all the same.
///
/// # Safety
/// `ptr` must be null or point at an object with a readable W# header.
pub unsafe fn is_collectable(ptr: *const u8) -> bool {
    !ptr.is_null() && !unsafe { test_flag(ptr, FLAG_IMMORTAL) }
}

// ---------------------------------------------------------------------------
// Thread-local allocation buffers
// ---------------------------------------------------------------------------

/// A run of bytes one thread may allocate out of without taking the heap lock.
///
/// Its block is `Open` for as long as the buffer holds it, which is what keeps
/// the unused tail from being handed to anyone else, the block from being
/// recycled under it, and a trace from choosing it for evacuation.
///
/// A struct of cells rather than a cell of a struct: allocation reads and
/// writes three of these words and never looks at the rest, and copying the
/// whole thing in and out on every allocation cost more than the bookkeeping
/// it carries.
struct Tlab {
    cursor: Cell<usize>,
    limit: Cell<usize>,
    /// Allocations the shared statistics have not been told about yet.
    ///
    /// Kept here rather than published per object because the statistics are
    /// four counters and publishing them is four atomic read-modify-writes --
    /// which, on a path this hot, cost more than the lock this buffer exists
    /// to avoid. They are flushed whenever anything could look: when the
    /// buffer is replaced or given back, and before the counters are read.
    pending_objects: Cell<usize>,
    pending_bytes: Cell<usize>,
    /// Sound for as long as the space lives, which for the process-wide heap
    /// is for ever. Buffers are only ever handed out by that heap.
    meta: Cell<*const SpaceMeta>,
    counters: Cell<*const Counters>,
    /// Recorded rather than derived from the cursor: an exhausted buffer's
    /// cursor sits at its limit, which may be the first byte of the *next*
    /// block.
    block: Cell<usize>,
}

thread_local! {
    static TLAB: Tlab = const {
        Tlab {
            cursor: Cell::new(0),
            limit: Cell::new(0),
            pending_objects: Cell::new(0),
            pending_bytes: Cell::new(0),
            meta: Cell::new(ptr::null()),
            counters: Cell::new(ptr::null()),
            block: Cell::new(0),
        }
    };
}

/// Allocate out of this thread's buffer, or report that it does not fit.
///
/// Takes no lock: the header, the object-start bit and the line counts are
/// published atomically, in that order, so a collector walking the heap never
/// sees a bit whose header has not been written.
fn tlab_alloc(type_id: TypeId, size: usize) -> Option<*mut u8> {
    TLAB.with(|tlab| {
        let addr = tlab.cursor.get();
        if addr + size > tlab.limit.get() {
            return None;
        }
        tlab.cursor.set(addr + size);
        tlab.pending_objects.set(tlab.pending_objects.get() + 1);
        tlab.pending_bytes.set(tlab.pending_bytes.get() + size);

        // Sound: the buffer was handed out by the process-wide heap, whose
        // spaces outlive the process.
        let meta = unsafe { &*tlab.meta.get() };
        let ptr = addr as *mut u8;
        stamp(ptr, type_id);
        meta.set_start(addr);
        meta.occupy(addr, size);
        Some(ptr)
    })
}

/// Tell the shared statistics about the allocations this thread has made since
/// they last heard. Cheap and idempotent; a thread with no buffer does nothing.
fn flush_tlab_counters() {
    TLAB.with(|tlab| {
        let objects = tlab.pending_objects.get();
        let counters = tlab.counters.get();
        if objects == 0 || counters.is_null() {
            return;
        }
        // Sound for the same reason `tlab_alloc`'s use is.
        unsafe { &*counters }.note_allocated_many(objects, tlab.pending_bytes.get());
        tlab.pending_objects.set(0);
        tlab.pending_bytes.set(0);
    });
}

/// Give up whatever is left of this thread's buffer, so that the block holding
/// it stops being `Open` and can be swept, recycled or evacuated.
fn retire_tlab(heap: &mut Heap) {
    flush_tlab_counters();
    let (meta, block) = TLAB.with(|tlab| {
        let meta = tlab.meta.replace(ptr::null());
        tlab.cursor.set(0);
        tlab.limit.set(0);
        (meta, tlab.block.get())
    });
    if !meta.is_null() {
        heap.close_block(unsafe { &*meta }, block);
    }
}

/// Write the header of a freshly allocated object.
fn stamp(ptr: *mut u8, type_id: TypeId) {
    // Born logged: a new object's fields are all null, so the write barrier
    // has nothing to record about it and can skip its slow path entirely.
    // Born with a count of zero: LXR counts *heap* references, and a new
    // object is so far reachable only from the stack. Born marked: a
    // snapshot-at-the-beginning trace treats everything allocated after its
    // snapshot as live, and the parity bit is how it says so.
    //
    // A release store, not a plain write: a collector thread that reads a
    // pointer to this object out of a field must see a stamped header, and
    // the field store itself is a plain store in generated code.
    unsafe { store_meta(ptr, meta_word(type_id, FLAG_LOGGED) | mark_parity()) };
}

// ---------------------------------------------------------------------------
// The heap
// ---------------------------------------------------------------------------

struct LargeObject {
    ptr: *mut u8,
    layout: Layout,
}

/// Where the heap's own lock-held allocation is bumping. Used for evacuation
/// copies and by the private heaps unit tests build; ordinary program
/// allocation goes through a thread's buffer instead.
#[derive(Clone, Copy)]
struct Bump {
    space: usize,
    block: usize,
    cursor: usize,
    limit: usize,
}

struct Space {
    meta: Box<SpaceMeta>,
    layout: Layout,
}

impl Space {
    fn new() -> Space {
        // Aligning the whole reservation to the block size is what makes
        // `(addr - base) >> BLOCK_BITS` a valid block index.
        let layout =
            Layout::from_size_align(SPACE_BYTES, BLOCK_BYTES).expect("valid heap space layout");
        // Zeroed so a partially initialised object's fields read as null,
        // which keeps it safe to trace at any moment. Holes are zeroed again
        // when they are refilled, for the same reason.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Space {
            meta: Box::new(SpaceMeta {
                base: ptr as usize,
                states: (0..SPACE_BLOCKS)
                    .map(|_| AtomicU8::new(BlockState::Free as u8))
                    .collect(),
                line_objects: (0..SPACE_BLOCKS * LINES_PER_BLOCK)
                    .map(|_| AtomicU8::new(0))
                    .collect(),
                live_lines: (0..SPACE_BLOCKS).map(|_| AtomicU16::new(0)).collect(),
                starts: (0..SPACE_BLOCKS * BITMAP_WORDS_PER_BLOCK)
                    .map(|_| AtomicU64::new(0))
                    .collect(),
            }),
            layout,
        }
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        unsafe { dealloc(self.meta.base as *mut u8, self.layout) };
    }
}

pub struct Heap {
    spaces: Vec<Space>,
    bump: Option<Bump>,
    large: Vec<LargeObject>,
    counters: &'static Counters,
    /// Whether new spaces go into the lock-free directory, and whether threads
    /// may take allocation buffers out of this heap. True for the process-wide
    /// heap; false for the private instances unit tests build, whose spaces are
    /// freed again and must never be findable afterwards.
    shared: bool,
}

// The heap hands out raw pointers into memory it owns for the lifetime of the
// process; the `Mutex` makes concurrent access to the bump pointer safe.
unsafe impl Send for Heap {}

impl Heap {
    /// One worker's heap. Publishes its spaces into the shared directory, so
    /// that the load barrier can answer questions about any address, and hands
    /// out allocation buffers to the threads that belong to it.
    pub(crate) fn new_worker(counters: &'static Counters) -> Heap {
        Heap {
            spaces: Vec::new(),
            bump: None,
            large: Vec::new(),
            counters,
            shared: true,
        }
    }

    /// A heap of its own, for a unit test: not published, so nothing else can
    /// find its objects, and with its own statistics.
    #[cfg(test)]
    fn new_private() -> Heap {
        Heap {
            spaces: Vec::new(),
            bump: None,
            large: Vec::new(),
            counters: Box::leak(Box::new(Counters::new())),
            shared: false,
        }
    }

    /// Hand a block back once no allocator is using it: `Free` if nothing is
    /// left in it, `Recyclable` if it has holes to refill, `Full` otherwise.
    fn close_block(&mut self, meta: &SpaceMeta, block: usize) {
        if meta.state(block) != BlockState::Open {
            return;
        }
        if meta.live_lines(block) == 0 {
            meta.clear_block_starts(block);
            meta.set_state(block, BlockState::Free);
        } else if meta.find_hole(block, 0, ALIGN).is_some() {
            meta.set_state(block, BlockState::Recyclable);
        } else {
            meta.set_state(block, BlockState::Full);
        }
    }

    /// Zero a hole before anything is allocated out of it.
    ///
    /// A hole in a block that is still in use holds the bytes of the objects
    /// that died there. A fresh object's fields must read as null -- the write
    /// barrier's slow path and the marker both read them before its
    /// initialising stores have run -- so the bytes go before the hole does.
    fn open_hole(&mut self, space: usize, block: usize, from: usize, to: usize) -> Bump {
        unsafe { ptr::write_bytes(from as *mut u8, 0, to - from) };
        let meta = &self.spaces[space].meta;
        meta.set_state(block, BlockState::Open);
        Bump {
            space,
            block,
            cursor: from,
            limit: to,
        }
    }

    /// Find somewhere with `want` bytes of room: the next hole in the block
    /// being bumped into, then a recyclable block, then a free one, then a new
    /// space. Returns the reservation, which the caller either bumps into or
    /// hands to a thread as its buffer.
    fn reserve(&mut self, want: usize) -> Bump {
        // The rest of the block already in hand.
        if let Some(bump) = self.bump {
            let meta_ptr: *const SpaceMeta = &*self.spaces[bump.space].meta;
            let meta = unsafe { &*meta_ptr };
            let next_line = (bump.limit - meta.block_start(bump.block)) / LINE_BYTES;
            if let Some((from, to)) = meta.find_hole(bump.block, next_line, want) {
                return self.open_hole(bump.space, bump.block, from, to);
            }
            self.close_block(meta, bump.block);
            self.bump = None;
        }

        // A block with a hole big enough, then one with nothing in it at all.
        for wanted in [BlockState::Recyclable, BlockState::Free] {
            for s in 0..self.spaces.len() {
                let meta_ptr: *const SpaceMeta = &*self.spaces[s].meta;
                let meta = unsafe { &*meta_ptr };
                for b in 0..SPACE_BLOCKS {
                    if meta.state(b) != wanted {
                        continue;
                    }
                    if let Some((from, to)) = meta.find_hole(b, 0, want) {
                        return self.open_hole(s, b, from, to);
                    }
                }
            }
        }

        let space = Space::new();
        if self.shared {
            publish(&space.meta);
        }
        self.spaces.push(space);
        let s = self.spaces.len() - 1;
        let start = self.spaces[s].meta.block_start(0);
        self.open_hole(s, 0, start, start + BLOCK_BYTES)
    }

    /// Hand this thread a buffer to allocate out of without the lock.
    ///
    /// It gets the whole hole the reservation found, however small: a hole of
    /// one line is still several objects, and refusing it would leave exactly
    /// the fragments that refilling holes exists to reclaim.
    fn refill_tlab(&mut self, want: usize) -> bool {
        if !self.shared {
            return false;
        }
        retire_tlab(self);
        let bump = self.reserve(want);
        // The heap's own bump position does not follow a buffer: the block is
        // the thread's until it gives it back.
        self.bump = None;
        let meta: *const SpaceMeta = &*self.spaces[bump.space].meta;
        TLAB.with(|tlab| {
            tlab.cursor.set(bump.cursor);
            tlab.limit.set(bump.limit);
            tlab.block.set(bump.block);
            tlab.meta.set(meta);
            tlab.counters.set(self.counters);
        });
        true
    }

    fn alloc_large(&mut self, type_id: TypeId, size: usize) -> *mut u8 {
        let layout = Layout::from_size_align(size, ALIGN).expect("valid large object layout");
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        self.large.push(LargeObject { ptr, layout });
        stamp(ptr, type_id);
        self.counters.note_allocated(size);
        ptr
    }

    /// Allocate with the lock held: evacuation copies, and the private heaps
    /// unit tests build.
    fn alloc_locked(&mut self, type_id: TypeId, size: usize) -> *mut u8 {
        if size >= LARGE_OBJECT_BYTES {
            return self.alloc_large(type_id, size);
        }
        let bump = match self.bump {
            Some(b) if b.cursor + size <= b.limit => b,
            _ => self.reserve(size),
        };
        let addr = bump.cursor;
        self.bump = Some(Bump {
            cursor: addr + size,
            ..bump
        });
        let meta = &self.spaces[bump.space].meta;
        let ptr = addr as *mut u8;
        stamp(ptr, type_id);
        meta.set_start(addr);
        meta.occupy(addr, size);
        self.counters.note_allocated(size);
        ptr
    }

    /// Reclaim one object. Freeing an object twice is harmless: the counting
    /// collector and the sweeper both reach here, and the tombstone decides
    /// who was first.
    ///
    /// The object's lines go back when nothing else is on them, and its block
    /// becomes recyclable -- or free, when the last of it goes.
    fn free(&mut self, ptr: *mut u8, size: usize) {
        // Tombstone rather than erase: `free` is reached from two places and
        // the bit is what makes the second one a no-op.
        if !unsafe { set_flag(ptr, FLAG_DEAD) } {
            return;
        }
        let addr = ptr as usize;
        self.counters.note_freed(size);

        if let Some(index) = self.large.iter().position(|o| o.ptr == ptr) {
            let object = self.large.swap_remove(index);
            unsafe { dealloc(object.ptr, object.layout) };
            return;
        }

        let Some(s) = self.spaces.iter().position(|s| s.meta.contains(addr)) else {
            debug_assert!(false, "freeing a pointer the heap does not own");
            return;
        };
        let meta_ptr: *const SpaceMeta = &*self.spaces[s].meta;
        let meta = unsafe { &*meta_ptr };
        let block = meta.block_index(addr);
        // The object stops being one the heap walk will find.
        meta.clear_start(addr);
        let freed = meta.vacate(addr, size);
        if freed == 0 {
            return;
        }
        // A block an allocator is holding keeps its state: its unused tail is
        // spoken for, and a trace must not choose it. One being evacuated is
        // released whole by the trace, not a line at a time here.
        match meta.state(block) {
            BlockState::Full | BlockState::Recyclable => {
                if meta.live_lines(block) == 0 {
                    meta.clear_block_starts(block);
                    meta.set_state(block, BlockState::Free);
                } else {
                    meta.set_state(block, BlockState::Recyclable);
                }
            }
            _ => {}
        }
    }

    /// Visit every live object in the heap, in address order, skipping the
    /// blocks being evacuated if asked (their contents have been copied out,
    /// and their headers may already be forwarding words).
    fn for_each_object(&self, skip_evacuating: bool, mut visit: impl FnMut(*mut u8)) {
        for space in &self.spaces {
            for b in 0..SPACE_BLOCKS {
                match space.meta.state(b) {
                    BlockState::Free => continue,
                    BlockState::Evacuating if skip_evacuating => continue,
                    _ => {}
                }
                space.meta.for_each_in_block(b, |ptr, _| {
                    if !unsafe { test_flag(ptr, FLAG_DEAD) } {
                        visit(ptr);
                    }
                });
            }
        }
        for object in &self.large {
            if !unsafe { test_flag(object.ptr, FLAG_DEAD) } {
                visit(object.ptr);
            }
        }
    }

    /// Free every live object in block `b` of space `s` that `is_garbage` says
    /// so of. Returns how many. One call holds the lock for one block, which
    /// bounds how long the allocator can be kept waiting by a sweep.
    fn sweep_block(&mut self, s: usize, b: usize, is_garbage: &dyn Fn(*mut u8) -> bool) -> usize {
        let Some(space) = self.spaces.get(s) else {
            return 0;
        };
        // Re-read under the lock: the mutator may have taken, filled or given
        // back this block since the sweeper last looked. A block an allocator
        // holds is left alone -- its objects are all new, and so all marked.
        if !matches!(
            space.meta.state(b),
            BlockState::Full | BlockState::Recyclable
        ) {
            return 0;
        }
        let meta_ptr: *const SpaceMeta = &*space.meta;
        let mut garbage: Vec<(*mut u8, u32)> = Vec::new();
        unsafe { &*meta_ptr }.for_each_in_block(b, |ptr, size| {
            if !unsafe { test_flag(ptr, FLAG_DEAD) } && is_garbage(ptr) {
                garbage.push((ptr, size));
            }
        });
        for &(ptr, size) in &garbage {
            self.free(ptr, size as usize);
        }
        garbage.len()
    }

    /// The large-object half of a sweep.
    fn sweep_large(&mut self, is_garbage: &dyn Fn(*mut u8) -> bool) -> usize {
        let garbage: Vec<(*mut u8, usize)> = self
            .large
            .iter()
            .filter(|o| !unsafe { test_flag(o.ptr, FLAG_DEAD) } && is_garbage(o.ptr))
            .map(|o| (o.ptr, o.layout.size()))
            .collect();
        for &(ptr, size) in &garbage {
            self.free(ptr, size);
        }
        garbage.len()
    }

    /// Mark the sparsest blocks for evacuation, returning how many.
    ///
    /// Refilling holes recovers the space around a survivor, but not the
    /// survivor's own line, and a block held down by a handful of objects
    /// scattered across it stays mostly unusable. Moving those few out is what
    /// recovers it, and it is the reason the header has a forwarding encoding
    /// at all.
    ///
    /// A block an allocator is holding is never a candidate: its tail is
    /// spoken for, and copies have to go somewhere.
    fn select_evacuation(&mut self, max_live_lines: u16) -> usize {
        let mut chosen = 0;
        for space in &self.spaces {
            for b in 0..SPACE_BLOCKS {
                let live = space.meta.live_lines(b);
                let sparse = live > 0 && live <= max_live_lines;
                let usable = matches!(
                    space.meta.state(b),
                    BlockState::Full | BlockState::Recyclable
                );
                if sparse && usable {
                    space.meta.set_state(b, BlockState::Evacuating);
                    chosen += 1;
                }
            }
        }
        chosen
    }

    /// Copy the objects `is_live` approves out of one block being evacuated,
    /// leaving a forwarding word in each old header, and collect the copies.
    ///
    /// One block per call, so the collector thread can do this a block at a
    /// time while the program runs: the lock is held for a block, never for
    /// the whole heap. The program may reach one of these objects first and
    /// move it itself, through the load barrier; whoever wins the forwarding
    /// word decides, and the loser's work is discarded.
    fn evacuate_block(
        &mut self,
        s: usize,
        b: usize,
        is_live: &dyn Fn(*mut u8) -> bool,
        copies: &mut Vec<*mut u8>,
    ) -> usize {
        if self.spaces[s].meta.state(b) != BlockState::Evacuating {
            return 0;
        }
        let meta_ptr: *const SpaceMeta = &*self.spaces[s].meta;
        let mut objects: Vec<(*mut u8, u32)> = Vec::new();
        unsafe { &*meta_ptr }.for_each_in_block(b, |ptr, size| {
            // A forwarded object has already gone; a dead one is not going.
            let meta = unsafe { crate::header::load_meta(ptr) };
            if !crate::header::is_forwarded_meta(meta)
                && !unsafe { test_flag(ptr, FLAG_DEAD) }
                && is_live(ptr)
            {
                objects.push((ptr, size));
            }
        });
        let mut moved = 0;
        for &(ptr, size) in &objects {
            let type_id = unsafe { crate::header::type_id_of(ptr) };
            let copy = self.alloc_locked(type_id, size as usize);
            unsafe { ptr::copy_nonoverlapping(ptr, copy, size as usize) };
            match unsafe { crate::header::try_forward(ptr, copy) } {
                Ok(()) => {
                    // The original is leaving with its block; the copy took
                    // its place on the live count a moment ago.
                    self.counters.note_freed(size as usize);
                    copies.push(copy);
                    moved += 1;
                }
                // The program got there first through the load barrier.
                Err(_) => self.free(copy, size as usize),
            }
        }
        moved
    }

    /// Account for the garbage about to go with the evacuated blocks.
    ///
    /// An object that was copied out was accounted for when it was forwarded,
    /// and one that was already dead when it was freed. What is left is what
    /// the trace found unreachable, which has never been taken off the count.
    /// Forwarding is tested first and nothing else is read from those headers:
    /// a forwarded word's bits are an address, not a size and not a flag.
    fn note_evacuated_blocks(&mut self) {
        for s in 0..self.spaces.len() {
            for b in 0..SPACE_BLOCKS {
                if self.spaces[s].meta.state(b) != BlockState::Evacuating {
                    continue;
                }
                let meta_ptr: *const SpaceMeta = &*self.spaces[s].meta;
                let mut gone: Vec<u32> = Vec::new();
                unsafe { &*meta_ptr }.for_each_in_block(b, |ptr, size| {
                    let meta = unsafe { crate::header::load_meta(ptr) };
                    if !crate::header::is_forwarded_meta(meta)
                        && !unsafe { test_flag(ptr, FLAG_DEAD) }
                    {
                        gone.push(size);
                    }
                });
                for size in gone {
                    self.counters.note_freed(size as usize);
                }
            }
        }
    }

    /// Return every evacuated block to the free list, whole. Returns how many.
    fn release_evacuated(&mut self) -> usize {
        let mut released = 0;
        for space in &self.spaces {
            for b in 0..SPACE_BLOCKS {
                if space.meta.state(b) != BlockState::Evacuating {
                    continue;
                }
                let base = b * LINES_PER_BLOCK;
                for line in &space.meta.line_objects[base..base + LINES_PER_BLOCK] {
                    line.store(0, Ordering::Relaxed);
                }
                space.meta.live_lines[b].store(0, Ordering::Relaxed);
                space.meta.clear_block_starts(b);
                space.meta.set_state(b, BlockState::Free);
                released += 1;
            }
        }
        released
    }

    /// Un-select every evacuation candidate. For a trace that is being given
    /// up on before it copied anything.
    fn revert_evacuation(&mut self) {
        for space in &self.spaces {
            for b in 0..SPACE_BLOCKS {
                if space.meta.state(b) == BlockState::Evacuating {
                    space.meta.set_state(b, BlockState::Full);
                }
            }
        }
    }

    pub fn stats(&self) -> HeapStats {
        HeapStats {
            bytes_allocated: self.counters.bytes_allocated.load(Ordering::Relaxed),
            objects_allocated: self.counters.objects_allocated.load(Ordering::Relaxed),
            live_objects: self.counters.live_objects.load(Ordering::Relaxed),
            live_bytes: self.counters.live_bytes.load(Ordering::Relaxed),
            blocks: self
                .spaces
                .iter()
                .map(|s| {
                    (0..SPACE_BLOCKS)
                        .filter(|&b| s.meta.state(b) != BlockState::Free)
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

/// Add a space to the lock-free directory. Called under the heap lock, so
/// there is one writer; the count goes last so a reader never sees an entry
/// that is not yet there.
fn publish(meta: &SpaceMeta) {
    let index = PUBLISHED_COUNT.load(Ordering::Relaxed);
    if index >= MAX_SPACES {
        eprintln!("W# heap: out of address space ({MAX_SPACES} spaces of {SPACE_BYTES} bytes)");
        std::process::abort();
    }
    PUBLISHED[index].store(
        meta as *const SpaceMeta as *mut SpaceMeta,
        Ordering::Relaxed,
    );
    PUBLISHED_COUNT.store(index + 1, Ordering::Release);
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

/// Run `f` on the heap of the worker this thread belongs to.
///
/// The one place the heap is reached, which is what made splitting it per
/// worker a change to this function rather than to its seventeen callers.
fn with_heap<R>(f: impl FnOnce(&mut Heap) -> R) -> R {
    let worker = crate::worker::Worker::current();
    f(&mut worker.heap.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Allocate a zeroed object of `size` bytes (header included), stamp its
/// header with `type_id`, and record `aux` as its element count.
///
/// This is the single allocation entry point for generated code. The common
/// case takes no lock: it is a bump and a few atomic updates in this thread's
/// own buffer.
///
/// `aux` is what makes a variable-sized object -- a string, an array -- know
/// how big it is, and it is written *here* rather than by the caller for one
/// reason: [`crate::gc::on_allocation`] below is a safepoint that can run a
/// whole collection, and until `aux` holds the count `object_size` reports the
/// object as a bare header. A heap walk during that collection would then step
/// into the middle of it. Fixed-size types pass zero.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `size` must include
/// [`HEADER_SIZE`] and match the registered layout for `type_id`.
#[unsafe(no_mangle)]
pub extern "C" fn ws_alloc(type_id: TypeId, size: u64, aux: u64) -> *mut u8 {
    let size = align_up((size as u32).max(HEADER_SIZE)) as usize;
    let ptr = allocate(type_id, size);
    if aux != 0 {
        unsafe { (ptr.offset(crate::header::AUX_OFFSET as isize) as *mut u64).write(aux) };
    }
    // The header was published above with a release store, but the zeroed
    // fields were written with plain stores. Generated code will store this
    // pointer into other objects' fields with plain stores too, and a collector
    // thread reading such a field must then see zeroes, never a previous
    // occupant's bytes. Free on x86-64; one barrier on aarch64.
    std::sync::atomic::fence(Ordering::Release);
    // A call is a safepoint, so this is one of the program points a collection
    // can actually happen at -- which is why it is also where the stack maps
    // are checked under `--gc-stress`. The frame chain above us is intact here.
    unsafe { crate::gc::on_allocation(ptr) };
    ptr
}

/// The allocation path proper: this thread's buffer, then the lock.
fn allocate(type_id: TypeId, size: usize) -> *mut u8 {
    if let Some(ptr) = tlab_alloc(type_id, size) {
        return ptr;
    }
    if size >= LARGE_OBJECT_BYTES {
        return with_heap(|heap| heap.alloc_large(type_id, size));
    }
    // A buffer too small for this object is replaced rather than topped up.
    // A heap that hands out no buffers -- the private ones unit tests build --
    // says so, and the allocation goes under the lock instead.
    if with_heap(|heap| heap.refill_tlab(size))
        && let Some(ptr) = tlab_alloc(type_id, size)
    {
        return ptr;
    }
    with_heap(|heap| heap.alloc_locked(type_id, size))
}

/// Space for a copy of an object being evacuated, from wherever this thread is
/// allocating.
///
/// Deliberately not [`ws_alloc`]: a copy is not a new object. It must not be
/// enrolled in the nursery, must not count towards the next collection, and
/// must not trigger one -- the thread calling this is in the middle of a load.
pub fn alloc_copy_shared(type_id: TypeId, size: u32) -> *mut u8 {
    allocate(type_id, align_up(size.max(HEADER_SIZE)) as usize)
}

/// Reclaim an object the collector has proved dead.
///
/// # Safety
/// `ptr` must be an object this heap allocated and `size` its allocated size.
/// Nothing may reference it afterwards.
pub unsafe fn free_object(ptr: *mut u8, size: u32) {
    with_heap(|heap| heap.free(ptr, align_up(size.max(HEADER_SIZE)) as usize));
}

/// Every worker's heap added together, for the exit report.
///
/// A program's heap is all of its workers' heaps: no object is in two of them,
/// so adding them up double-counts nothing.
pub fn total_heap_stats() -> HeapStats {
    let mut total = HeapStats::default();
    flush_tlab_counters();
    crate::worker::for_each_worker(|w| {
        let s = w.heap.lock().unwrap_or_else(|e| e.into_inner()).stats();
        total.bytes_allocated += s.bytes_allocated;
        total.objects_allocated += s.objects_allocated;
        total.live_objects += s.live_objects;
        total.live_bytes += s.live_bytes;
        total.blocks += s.blocks;
        total.large_objects += s.large_objects;
    });
    total
}

pub fn heap_stats() -> HeapStats {
    // Whatever this thread has allocated but not yet published would otherwise
    // be missing from the answer. A thread without a buffer -- the collector --
    // has nothing to add, and reads what the mutator last published.
    flush_tlab_counters();
    with_heap(|heap| heap.stats())
}

/// Publish the calling thread's pending allocation statistics.
pub fn flush_local_counters() {
    flush_tlab_counters();
}

/// Give up the calling thread's allocation buffer, so the block holding it can
/// be swept, recycled or evacuated. Called in a trace's pauses, on the thread
/// whose buffer it is.
pub fn retire_local_buffer() {
    with_heap(retire_tlab);
}

/// Visit every live object, under the heap lock. For the trace's pauses only:
/// the sweeper and the allocator are both waiting while this runs.
pub fn for_each_object(skip_evacuating: bool, visit: impl FnMut(*mut u8)) {
    with_heap(|heap| heap.for_each_object(skip_evacuating, visit));
}

/// How many spaces exist right now. Spaces are only added, so a sweep that
/// reads this once and walks that many is complete for everything that
/// existed when it started -- and anything newer holds only new objects.
pub fn space_count() -> usize {
    with_heap(|heap| heap.spaces.len())
}

pub fn block_count(_space: usize) -> usize {
    SPACE_BLOCKS
}

/// Free the objects in one block that `is_garbage` condemns. Takes and
/// releases the heap lock, so a sweep can interleave with the allocator.
pub fn sweep_block(space: usize, block: usize, is_garbage: &dyn Fn(*mut u8) -> bool) -> usize {
    with_heap(|heap| heap.sweep_block(space, block, is_garbage))
}

pub fn sweep_large(is_garbage: &dyn Fn(*mut u8) -> bool) -> usize {
    with_heap(|heap| heap.sweep_large(is_garbage))
}

/// Choose the sparsest blocks for evacuation. Returns how many were chosen.
pub fn select_evacuation(max_live_lines: u16) -> usize {
    with_heap(|heap| heap.select_evacuation(max_live_lines))
}

/// Copy the survivors out of one block being evacuated. Returns how many
/// moved, and appends each copy to `copies` so the fix-up can revisit their
/// fields. Takes and releases the heap lock, so the program keeps running.
pub fn evacuate_block(
    space: usize,
    block: usize,
    is_live: &dyn Fn(*mut u8) -> bool,
    copies: &mut Vec<*mut u8>,
) -> usize {
    with_heap(|heap| heap.evacuate_block(space, block, is_live, copies))
}

/// Take the garbage left in the evacuated blocks off the live count.
pub fn note_evacuated_blocks() {
    with_heap(|heap| heap.note_evacuated_blocks());
}

/// Account for an object that has just been forwarded out of a block being
/// emptied: it is leaving, and its copy is already on the count.
pub fn note_forwarded(size: u32) {
    with_heap(|heap| {
        heap.counters
            .note_freed(align_up(size.max(HEADER_SIZE)) as usize)
    });
}

/// The blocks currently being evacuated, as `(space, block)` pairs.
pub fn evacuating_blocks() -> Vec<(usize, usize)> {
    with_heap(|heap| {
        let mut out = Vec::new();
        for (s, space) in heap.spaces.iter().enumerate() {
            for b in 0..SPACE_BLOCKS {
                if space.meta.state(b) == BlockState::Evacuating {
                    out.push((s, b));
                }
            }
        }
        out
    })
}

/// Return every evacuated block to the free list. Returns how many.
pub fn release_evacuated() -> usize {
    with_heap(|heap| heap.release_evacuated())
}

/// Give up on evacuating: every candidate goes back to being an ordinary block.
pub fn revert_evacuation() {
    with_heap(|heap| heap.revert_evacuation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{
        FLAG_LOGGED, TYPE_ID_FIRST_USER, flip_mark_parity, is_marked, test_flag, type_id_of,
    };
    use crate::test_support::SERIAL;

    /// Walking a block reads each object's size from the type registry, so the
    /// tests that walk register a type of the right size.
    fn registered(id: TypeId, size: u32) -> TypeId {
        crate::types::register_type(
            id,
            crate::types::TypeLayout::fixed(format!("heap test type {id}"), size, Vec::new()),
        );
        crate::types::publish();
        id
    }

    #[test]
    fn allocations_are_aligned_distinct_and_tagged() {
        let a = ws_alloc(TYPE_ID_FIRST_USER, 24, 0);
        let b = ws_alloc(TYPE_ID_FIRST_USER + 1, 24, 0);
        assert!(!a.is_null() && !b.is_null());
        assert_ne!(a, b);
        assert_eq!(a as usize % ALIGN, 0);
        assert_eq!(b as usize % ALIGN, 0);
        unsafe {
            assert_eq!(type_id_of(a), TYPE_ID_FIRST_USER);
            assert_eq!(type_id_of(b), TYPE_ID_FIRST_USER + 1);
        }
    }

    #[test]
    fn fields_start_zeroed() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        unsafe {
            assert_eq!((p.add(16) as *const u64).read(), 0);
            assert_eq!((p.add(24) as *const u64).read(), 0);
        }
    }

    #[test]
    fn new_objects_are_born_logged_and_marked() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // The write barrier's fast path is a test of the logged bit, so an
        // object whose fields are all still null must never take the slow
        // path; and a snapshot trace treats everything born after its
        // snapshot as live.
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        unsafe {
            assert!(test_flag(p, FLAG_LOGGED));
            assert!(is_marked(p));
        }
    }

    #[test]
    fn an_object_larger_than_a_block_still_allocates() {
        let big = (BLOCK_BYTES + 4096) as u64;
        let p = ws_alloc(TYPE_ID_FIRST_USER, big, 0);
        assert!(!p.is_null());
        assert_eq!(p as usize % ALIGN, 0);
        // Writing the last byte must not fault.
        unsafe { p.add(big as usize - 1).write(0xAB) };
        unsafe { assert_eq!(p.add(big as usize - 1).read(), 0xAB) };
        // It went to the large-object space, not into a block -- and is
        // collectable all the same.
        assert!(!in_heap(p));
        assert!(unsafe { is_collectable(p) });
    }

    #[test]
    fn a_zero_size_request_still_gets_a_header() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 0, 0);
        let q = ws_alloc(TYPE_ID_FIRST_USER, 0, 0);
        assert_ne!(p, q);
        assert!((q as usize).abs_diff(p as usize) >= HEADER_SIZE as usize);
    }

    #[test]
    fn block_space_objects_are_recognised_and_others_are_not() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        assert!(in_heap(p), "a normal object lives in a block");
        assert!(!is_evacuating(p), "nothing is being evacuated");

        // A static object, as the code generator emits for a string literal or
        // a zero-field struct's singleton. It must not look like ours, and it
        // must not be collectable.
        #[repr(align(16))]
        // The payload exists to be pointed at, not read.
        #[allow(dead_code)]
        struct Static([u64; 2]);
        let s = Static([meta_word(TYPE_ID_FIRST_USER, FLAG_IMMORTAL), 0]);
        let sp = &s as *const Static as *const u8;
        assert!(!in_heap(sp));
        assert!(!unsafe { is_collectable(sp) });
        assert!(!unsafe { is_collectable(ptr::null()) });
    }

    #[test]
    fn allocation_spans_more_than_one_block() {
        // Enough to fill several blocks, proving a new block is opened rather
        // than the cursor running off the end of the first.
        let before = heap_stats();
        let count = (BLOCK_BYTES / 64) * 3;
        let mut last = ptr::null_mut();
        for _ in 0..count {
            last = ws_alloc(TYPE_ID_FIRST_USER, 64, 0);
            assert!(!last.is_null());
        }
        let after = heap_stats();
        assert!(after.blocks > before.blocks, "more blocks were opened");
        // `>=` rather than `==`: the heap is process-global and the test
        // binary runs its tests in parallel, so others allocate too.
        assert!(after.objects_allocated - before.objects_allocated >= count);
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
            let p = ws_alloc(TYPE_ID_FIRST_USER, size, 0) as usize;
            let start_block = p >> BLOCK_BITS;
            let end_block = (p + size as usize - 1) >> BLOCK_BITS;
            assert_eq!(start_block, end_block, "object straddled a block");
        }
    }

    // The tests below drive a private heap, so they can free, sweep and
    // evacuate without touching objects other tests hold.

    #[test]
    fn freeing_twice_is_harmless() {
        let ty = registered(TYPE_ID_FIRST_USER + 402, 32);
        let mut heap = Heap::new_private();
        let p = heap.alloc_locked(ty, 32);
        let q = heap.alloc_locked(ty, 32);
        assert_eq!(heap.stats().live_objects, 2);
        heap.free(p, 32);
        heap.free(p, 32);
        assert_eq!(
            heap.stats().live_objects,
            1,
            "the second free changed nothing"
        );
        assert!(unsafe { test_flag(p, FLAG_DEAD) });
        assert!(!unsafe { test_flag(q, FLAG_DEAD) });
    }

    #[test]
    fn the_heap_walk_finds_exactly_the_live_objects() {
        // The bitmap, not a stride walk: a freed object in the middle must
        // neither be visited nor stop the walk before the ones after it.
        let ty = registered(TYPE_ID_FIRST_USER + 403, 64);
        let mut heap = Heap::new_private();
        let all: Vec<*mut u8> = (0..8).map(|_| heap.alloc_locked(ty, 64)).collect();
        for &p in &[all[1], all[2], all[5]] {
            heap.free(p, 64);
        }
        let mut seen = Vec::new();
        heap.for_each_object(false, |p| seen.push(p));
        assert_eq!(seen, vec![all[0], all[3], all[4], all[6], all[7]]);
    }

    #[test]
    fn a_hole_in_a_block_still_holding_objects_is_refilled() {
        // The point of partial reuse: a block keeps one survivor and hands
        // back the lines around it, without waiting for an evacuation.
        let size = LINE_BYTES;
        let ty = registered(TYPE_ID_FIRST_USER + 404, size as u32);
        let mut heap = Heap::new_private();
        let per_block = BLOCK_BYTES / size;
        let first: Vec<*mut u8> = (0..per_block)
            .map(|_| heap.alloc_locked(ty, size))
            .collect();
        // Force the allocator off this block, then empty all but one line.
        let elsewhere = heap.alloc_locked(ty, size);
        assert_ne!(
            (elsewhere as usize) >> BLOCK_BITS,
            (first[0] as usize) >> BLOCK_BITS
        );
        for &p in &first[1..] {
            heap.free(p, size);
        }
        let meta = &heap.spaces[0].meta;
        let block = meta.block_index(first[0] as usize);
        assert_eq!(meta.state(block), BlockState::Recyclable);
        assert_eq!(meta.live_lines(block), 1);

        // Send the allocator looking for somewhere to go. It fills the block
        // it already holds before considering any other, so the preference
        // being tested here -- a block with holes over a block with nothing in
        // it -- only shows once it has to choose.
        heap.bump = None;
        let refilled: Vec<*mut u8> = (0..per_block - 1)
            .map(|_| heap.alloc_locked(ty, size))
            .collect();
        assert!(
            refilled
                .iter()
                .all(|&p| (p as usize) >> BLOCK_BITS == (first[0] as usize) >> BLOCK_BITS),
            "the emptied lines were reused"
        );
        assert!(
            !refilled.contains(&first[0]),
            "the survivor's own line was not handed out"
        );
        // Every refilled object reads as null, not as its predecessor's bytes.
        for &p in &refilled {
            unsafe { assert_eq!((p.add(HEADER_SIZE as usize) as *const u64).read(), 0) };
        }
    }

    #[test]
    fn a_thread_allocates_out_of_its_own_buffer_without_the_lock() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Two allocations from one buffer are adjacent, and the second one
        // took no lock: the buffer had already been handed out.
        let a = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        let b = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        let adjacent = (b as usize) == (a as usize) + 32;
        assert!(adjacent || (a as usize) >> BLOCK_BITS != (b as usize) >> BLOCK_BITS);
        assert!(TLAB.with(|t| t.limit.get()) > 0, "a buffer is held");

        // Retiring it gives the block back, and the objects in it survive.
        retire_local_buffer();
        assert_eq!(TLAB.with(|t| t.limit.get()), 0);
        assert!(in_heap(a) && in_heap(b));
        unsafe { assert_eq!(type_id_of(b), TYPE_ID_FIRST_USER) };
    }

    #[test]
    fn a_sweep_frees_only_objects_of_the_old_parity() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ty = registered(TYPE_ID_FIRST_USER + 400, 32);
        let mut heap = Heap::new_private();
        let old_a = heap.alloc_locked(ty, 32);
        let old_b = heap.alloc_locked(ty, 32);
        flip_mark_parity();
        let young = heap.alloc_locked(ty, 32);
        assert!(!unsafe { is_marked(old_a) });
        assert!(unsafe { is_marked(young) });

        // The sweeper only visits blocks no allocator is holding.
        let meta_ptr: *const SpaceMeta = &*heap.spaces[0].meta;
        let block = unsafe { &*meta_ptr }.block_index(old_a as usize);
        heap.bump = None;
        heap.close_block(unsafe { &*meta_ptr }, block);

        let is_garbage = |p: *mut u8| !unsafe { is_marked(p) };
        let freed = heap.sweep_block(0, block, &is_garbage);
        flip_mark_parity();

        assert_eq!(freed, 2);
        assert!(unsafe { test_flag(old_a, FLAG_DEAD) });
        assert!(unsafe { test_flag(old_b, FLAG_DEAD) });
        assert!(!unsafe { test_flag(young, FLAG_DEAD) });
        assert_eq!(heap.stats().live_objects, 1);
    }

    #[test]
    fn evacuation_copies_survivors_and_forwards_them() {
        let size = 1024usize;
        let ty = registered(TYPE_ID_FIRST_USER + 401, size as u32);
        let mut heap = Heap::new_private();
        let per_block = BLOCK_BYTES / size;
        let first: Vec<*mut u8> = (0..per_block)
            .map(|_| heap.alloc_locked(ty, size))
            .collect();
        let _elsewhere = heap.alloc_locked(ty, size);
        // Free all but one object in the first block: it is now sparse.
        for &p in &first[1..] {
            heap.free(p, size);
        }
        let survivor = first[0];
        unsafe { (survivor.add(HEADER_SIZE as usize) as *mut u64).write(0x5EED) };
        assert_eq!(heap.select_evacuation((LINES_PER_BLOCK / 4) as u16), 1);
        let before = heap.stats().live_objects;

        let mut copies = Vec::new();
        let block = heap.spaces[0].meta.block_index(first[0] as usize);
        let moved = heap.evacuate_block(0, block, &|_| true, &mut copies);
        assert_eq!(moved, 1);
        assert_eq!(copies.len(), 1);
        let meta = unsafe { crate::header::load_meta(survivor) };
        assert!(crate::header::is_forwarded_meta(meta));
        let copy = crate::header::forwarding_target(meta);
        assert_ne!(copy, survivor);
        assert_ne!(
            (copy as usize) >> BLOCK_BITS,
            (survivor as usize) >> BLOCK_BITS
        );
        unsafe {
            assert_eq!(
                (copy.add(HEADER_SIZE as usize) as *const u64).read(),
                0x5EED
            );
            assert_eq!(type_id_of(copy), ty);
        }
        // The survivor left with its block and came back as the copy.
        heap.note_evacuated_blocks();
        assert_eq!(heap.stats().live_objects, before);
        assert_eq!(heap.release_evacuated(), 1);
    }
}
