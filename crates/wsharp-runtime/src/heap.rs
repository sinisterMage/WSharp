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
//! * Side metadata -- a count per line, a state per block -- lives outside
//!   the objects, indexed by arithmetic on the address. That is why blocks come
//!   from *over-aligned* reservations: given any pointer, subtracting the
//!   space's base and shifting yields its block and line with no search.
//!
//! Objects too big for a quarter of a block go to a separate large-object
//! space. They are never moved and never share a block, so a 1 MiB array costs
//! one allocation rather than fragmenting the main space.
//!
//! Two consumers read this module, and they hold different locks. The mutator
//! allocates and the counting collector frees under the heap mutex. The
//! collector thread, while marking, asks only two questions -- is this address
//! in a block space, and is its block being evacuated -- and asks them per
//! reference, so those are answered from a lock-free directory of the spaces
//! ([`in_heap`], [`is_evacuating`]) rather than under the mutex.
//!
//! Reclamation policy lives in [`crate::gc`] and [`crate::mark`]: this module
//! knows how to free an object, sweep a block and recycle it, but not when to.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

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
    /// No live object; available for allocation.
    Free = 0,
    /// Being allocated into.
    Open = 1,
    /// Full, with live objects.
    Full = 2,
    /// Selected for evacuation by the trace in progress.
    Evacuating = 3,
}

impl BlockState {
    fn from_u8(v: u8) -> BlockState {
        match v {
            0 => BlockState::Free,
            1 => BlockState::Open,
            2 => BlockState::Full,
            _ => BlockState::Evacuating,
        }
    }
}

struct Block {
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
    layout: Layout,
    blocks: Vec<Block>,
    /// One state per block, kept apart from [`Block`] because the marker reads
    /// it without the heap lock while the allocator updates its neighbours
    /// under it. Never resized, so the directory can hold a pointer to it.
    states: Box<[AtomicU8]>,
    /// Live objects occupying each line, indexed
    /// `block * LINES_PER_BLOCK + line`. A count rather than a mark bit,
    /// because reclamation is per line and a line is only reusable once every
    /// object touching it is gone -- including one that started on the line
    /// before.
    line_objects: Vec<u8>,
}

impl Space {
    fn new() -> Space {
        // Aligning the whole reservation to the block size is what makes
        // `(addr - base) >> BLOCK_BITS` a valid block index.
        let layout =
            Layout::from_size_align(SPACE_BYTES, BLOCK_BYTES).expect("valid heap space layout");
        // Zeroed so a partially initialised object's fields read as null,
        // which keeps it safe to trace at any moment. Reopened blocks are
        // zeroed again in `open_block` for the same reason.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Space {
            base: ptr as usize,
            layout,
            blocks: (0..SPACE_BLOCKS)
                .map(|_| Block {
                    cursor: 0,
                    live_lines: 0,
                })
                .collect(),
            states: (0..SPACE_BLOCKS)
                .map(|_| AtomicU8::new(BlockState::Free as u8))
                .collect(),
            line_objects: vec![0; SPACE_BLOCKS * LINES_PER_BLOCK],
        }
    }

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
        BlockState::from_u8(self.states[block].load(Ordering::Relaxed))
    }

    fn set_state(&self, block: usize, state: BlockState) {
        self.states[block].store(state as u8, Ordering::Release);
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

    /// Bump `size` bytes out of block `b`, which must have room.
    fn bump(&mut self, b: usize, size: usize) -> usize {
        let addr = self.block_start(b) + self.blocks[b].cursor as usize;
        self.blocks[b].cursor += size as u32;
        // The collector derives an object's block by shifting its address, so
        // an object that straddled a boundary would be attributed to the wrong
        // one. `open_block` skips the tail of a block rather than splitting.
        debug_assert_eq!(self.block_index(addr), b);
        debug_assert_eq!(self.block_index(addr + size - 1), b);
        // Reclamation works a line at a time, so an object spanning two lines
        // pins both until it dies.
        let newly = self.occupy(addr, size);
        self.blocks[b].live_lines += newly;
        addr
    }

    /// Every object in block `b`, dead or alive, in address order.
    ///
    /// Blocks are filled by bumping, so their objects are laid end to end and a
    /// linear scan finds them all -- provided every object's size can be read
    /// back, which is why freeing tombstones a header rather than erasing it.
    /// Sizes are read before `visit` runs, so a visitor that forwards the
    /// object (which overwrites the type id) does not lose the walk its place.
    fn for_each_in_block(&self, b: usize, mut visit: impl FnMut(*mut u8, u32)) {
        let start = self.block_start(b);
        let end = start + self.blocks[b].cursor as usize;
        let mut addr = start;
        while addr < end {
            let ptr = addr as *mut u8;
            let Some(size) = (unsafe { crate::types::object_size(ptr) }) else {
                // An unregistered type id means the walk has lost its place;
                // stopping is safer than guessing a stride.
                break;
            };
            let size = align_up(size.max(HEADER_SIZE));
            visit(ptr, size);
            addr += size as usize;
        }
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        unsafe { dealloc(self.base as *mut u8, self.layout) };
    }
}

// ---------------------------------------------------------------------------
// The lock-free directory
// ---------------------------------------------------------------------------

/// One published space: where it starts, and where its block states are.
struct PublishedSpace {
    base: AtomicUsize,
    states: AtomicPtr<AtomicU8>,
}

/// The block spaces the process-wide heap has reserved, readable without the
/// heap lock. Spaces are only ever added, and never freed, so an entry, once
/// published, is good for the life of the process. The count is stored last,
/// with release ordering, which is what makes an entry complete when seen.
static PUBLISHED: [PublishedSpace; MAX_SPACES] = [const {
    PublishedSpace {
        base: AtomicUsize::new(0),
        states: AtomicPtr::new(ptr::null_mut()),
    }
}; MAX_SPACES];
static PUBLISHED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The published space holding `addr`, and the index of its block.
fn lookup(addr: usize) -> Option<(&'static PublishedSpace, usize)> {
    let count = PUBLISHED_COUNT.load(Ordering::Acquire);
    for entry in &PUBLISHED[..count] {
        let base = entry.base.load(Ordering::Relaxed);
        if addr.wrapping_sub(base) < SPACE_BYTES {
            return Some((entry, (addr - base) >> BLOCK_BITS));
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
    lookup(ptr as usize).is_some_and(|(space, block)| {
        let states = space.states.load(Ordering::Relaxed);
        // The pointer was published with the base and outlives the process.
        let state = unsafe { (*states.add(block)).load(Ordering::Acquire) };
        BlockState::from_u8(state) == BlockState::Evacuating
    })
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
// The heap
// ---------------------------------------------------------------------------

struct LargeObject {
    ptr: *mut u8,
    layout: Layout,
}

pub struct Heap {
    spaces: Vec<Space>,
    /// The block currently being bumped into, as `(space, block)`.
    open: Option<(usize, usize)>,
    large: Vec<LargeObject>,
    /// Whether new spaces go into the lock-free directory. True for the
    /// process-wide heap; false for the private instances unit tests build,
    /// whose spaces are freed again and must never be findable afterwards.
    publish: bool,
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
    const fn new(publish: bool) -> Heap {
        Heap {
            spaces: Vec::new(),
            open: None,
            large: Vec::new(),
            publish,
            bytes_allocated: 0,
            objects_allocated: 0,
            live_objects: 0,
            live_bytes: 0,
        }
    }

    /// Find a block with room, opening a new one -- and a new space, if need be.
    fn open_block(&mut self, size: usize) -> (usize, usize) {
        if let Some((s, b)) = self.open
            && self.spaces[s].blocks[b].cursor as usize + size <= BLOCK_BYTES
        {
            return (s, b);
        }
        if let Some((s, b)) = self.open {
            self.spaces[s].set_state(b, BlockState::Full);
        }
        for s in 0..self.spaces.len() {
            if let Some(b) =
                (0..SPACE_BLOCKS).find(|&b| self.spaces[s].state(b) == BlockState::Free)
            {
                // A recycled block still holds its previous objects' bytes, and
                // a fresh object's fields must read as null until its
                // initialising stores run: the write barrier's slow path and
                // the collector both read them, and either can run between
                // `ws_alloc` returning and those stores. The reservation was
                // zeroed when it was made; a block is zeroed again each time it
                // is reopened, which is the one place both recycling paths meet.
                let start = self.spaces[s].block_start(b);
                unsafe { ptr::write_bytes(start as *mut u8, 0, BLOCK_BYTES) };
                self.spaces[s].set_state(b, BlockState::Open);
                self.open = Some((s, b));
                return (s, b);
            }
        }
        let space = Space::new();
        if self.publish {
            publish(&space);
        }
        self.spaces.push(space);
        let s = self.spaces.len() - 1;
        self.spaces[s].set_state(0, BlockState::Open);
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
        // object is so far reachable only from the stack. Born marked: a
        // snapshot-at-the-beginning trace treats everything allocated after
        // its snapshot as live, and the parity bit is how it says so.
        //
        // A release store, not a plain write: a collector thread that reads a
        // pointer to this object out of a field must see a stamped header, and
        // the field store itself is a plain store in generated code.
        unsafe { store_meta(ptr, meta_word(type_id, FLAG_LOGGED) | mark_parity()) };
    }

    fn alloc(&mut self, type_id: TypeId, size: u32) -> *mut u8 {
        let size = align_up(size.max(HEADER_SIZE)) as usize;
        self.bytes_allocated += size;
        self.objects_allocated += 1;

        if size >= LARGE_OBJECT_BYTES {
            return self.alloc_large(type_id, size);
        }

        let (s, b) = self.open_block(size);
        let addr = self.spaces[s].bump(b, size);
        self.live_objects += 1;
        self.live_bytes += size;

        let ptr = addr as *mut u8;
        self.stamp(ptr, type_id);
        ptr
    }

    /// Reclaim one object. Freeing an object twice is harmless: the counting
    /// collector and the sweeper both reach here, and the tombstone decides
    /// who was first.
    ///
    /// Its lines go back when nothing else is on them, and a block whose lines
    /// are all empty is recycled whole. Note what this does *not* do: hand back
    /// the free lines of a block that still holds something. A single survivor
    /// pins its whole block, which is the fragmentation evacuation exists to
    /// fix.
    fn free(&mut self, ptr: *mut u8, size: usize) {
        // Tombstone rather than erase: a linear walk of the block needs the
        // type id to know how far to step, so the header stays readable and
        // only gains a bit saying the object is gone.
        if !unsafe { set_flag(ptr, FLAG_DEAD) } {
            return;
        }
        let addr = ptr as usize;
        self.live_objects -= 1;
        self.live_bytes -= size;

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
        // currently being bumped into is left alone (its cursor is live), and
        // so is one being evacuated: the trace releases those itself, whole.
        if space.blocks[b].live_lines == 0
            && space.state(b) == BlockState::Full
            && self.open != Some((s, b))
        {
            space.set_state(b, BlockState::Free);
            space.blocks[b].cursor = 0;
        }
    }

    /// Visit every live object in the heap, in address order, skipping the
    /// blocks being evacuated if asked (their contents have been copied out,
    /// and their headers may already be forwarding words).
    fn for_each_object(&self, skip_evacuating: bool, mut visit: impl FnMut(*mut u8)) {
        for space in &self.spaces {
            for b in 0..SPACE_BLOCKS {
                match space.state(b) {
                    BlockState::Free => continue,
                    BlockState::Evacuating if skip_evacuating => continue,
                    _ => {}
                }
                space.for_each_in_block(b, |ptr, _| {
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
        // Re-read everything under the lock: the mutator may have opened,
        // filled or recycled this block since the sweeper last looked.
        let Some(space) = self.spaces.get(s) else {
            return 0;
        };
        if matches!(space.state(b), BlockState::Free | BlockState::Evacuating)
            || space.blocks[b].cursor == 0
        {
            return 0;
        }
        let mut garbage: Vec<(*mut u8, u32)> = Vec::new();
        space.for_each_in_block(b, |ptr, size| {
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
        for (s, space) in self.spaces.iter().enumerate() {
            for b in 0..SPACE_BLOCKS {
                let live = space.blocks[b].live_lines;
                let sparse = live > 0 && live <= max_live_lines;
                if sparse && space.state(b) == BlockState::Full && open != Some((s, b)) {
                    space.set_state(b, BlockState::Evacuating);
                    chosen += 1;
                }
            }
        }
        chosen
    }

    /// Allocate space for a copy, without any of the collector bookkeeping a
    /// program allocation gets: a copy is not a new object, and must not be
    /// enrolled in the nursery or trigger a nested collection.
    fn alloc_copy(&mut self, size: usize) -> *mut u8 {
        debug_assert!(
            size < LARGE_OBJECT_BYTES,
            "large objects are never evacuated"
        );
        let (s, b) = self.open_block(size);
        let addr = self.spaces[s].bump(b, size);
        self.live_objects += 1;
        self.live_bytes += size;
        addr as *mut u8
    }

    /// Copy every object `is_live` approves out of the evacuating blocks and
    /// leave a forwarding word in each old header. Returns how many moved.
    ///
    /// Every object in those blocks is leaving with its block, copied or not,
    /// so all of them come off the live count; the copies go back on through
    /// `alloc_copy`. Sizes are read before the header is overwritten.
    fn evacuate(&mut self, is_live: &dyn Fn(*mut u8) -> bool) -> usize {
        let mut moved = 0;
        for s in 0..self.spaces.len() {
            for b in 0..SPACE_BLOCKS {
                if self.spaces[s].state(b) != BlockState::Evacuating {
                    continue;
                }
                let mut objects: Vec<(*mut u8, u32)> = Vec::new();
                self.spaces[s].for_each_in_block(b, |ptr, size| {
                    if !unsafe { test_flag(ptr, FLAG_DEAD) } {
                        objects.push((ptr, size));
                    }
                });
                for &(ptr, size) in &objects {
                    self.live_objects -= 1;
                    self.live_bytes -= size as usize;
                    if !is_live(ptr) {
                        continue;
                    }
                    let copy = self.alloc_copy(size as usize);
                    unsafe { ptr::copy_nonoverlapping(ptr, copy, size as usize) };
                    let claimed = unsafe { crate::header::try_forward(ptr, copy) };
                    debug_assert!(claimed.is_ok(), "an object was forwarded twice");
                    moved += 1;
                }
            }
        }
        moved
    }

    /// Return every evacuated block to the free list, whole. Returns how many.
    fn release_evacuated(&mut self) -> usize {
        let mut released = 0;
        for space in &mut self.spaces {
            for b in 0..SPACE_BLOCKS {
                if space.state(b) != BlockState::Evacuating {
                    continue;
                }
                let base = b * LINES_PER_BLOCK;
                for line in &mut space.line_objects[base..base + LINES_PER_BLOCK] {
                    *line = 0;
                }
                space.blocks[b].live_lines = 0;
                space.blocks[b].cursor = 0;
                space.set_state(b, BlockState::Free);
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
                if space.state(b) == BlockState::Evacuating {
                    space.set_state(b, BlockState::Full);
                }
            }
        }
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
                    (0..SPACE_BLOCKS)
                        .filter(|&b| s.state(b) != BlockState::Free)
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
/// there is one writer; the count goes last so a reader never sees a
/// half-written entry.
fn publish(space: &Space) {
    let index = PUBLISHED_COUNT.load(Ordering::Relaxed);
    if index >= MAX_SPACES {
        eprintln!("W# heap: out of address space ({MAX_SPACES} spaces of {SPACE_BYTES} bytes)");
        std::process::abort();
    }
    PUBLISHED[index].base.store(space.base, Ordering::Relaxed);
    PUBLISHED[index]
        .states
        .store(space.states.as_ptr() as *mut AtomicU8, Ordering::Relaxed);
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

static HEAP: Mutex<Heap> = Mutex::new(Heap::new(true));

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

/// Reclaim an object the collector has proved dead.
///
/// # Safety
/// `ptr` must be an object this heap allocated and `size` its allocated size.
/// Nothing may reference it afterwards.
pub unsafe fn free_object(ptr: *mut u8, size: u32) {
    with_heap(|heap| heap.free(ptr, align_up(size.max(HEADER_SIZE)) as usize));
}

pub fn heap_stats() -> HeapStats {
    with_heap(|heap| heap.stats())
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

/// Copy the surviving objects out of every evacuating block. Returns how many
/// moved. Only for the trace's final pause.
pub fn evacuate(is_live: &dyn Fn(*mut u8) -> bool) -> usize {
    with_heap(|heap| heap.evacuate(is_live))
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
    fn new_objects_are_born_logged_and_marked() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // The write barrier's fast path is a test of the logged bit, so an
        // object whose fields are all still null must never take the slow
        // path; and a snapshot trace treats everything born after its
        // snapshot as live.
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        unsafe {
            assert!(test_flag(p, FLAG_LOGGED));
            assert!(is_marked(p));
        }
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
        // It went to the large-object space, not into a block -- and is
        // collectable all the same.
        assert!(!in_heap(p));
        assert!(unsafe { is_collectable(p) });
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

    // The tests below drive a private heap, so they can free and sweep without
    // touching objects other tests hold. Walking a block reads each object's
    // size from the type registry, so they register a type of the right size.
    fn registered(id: TypeId, size: u32) -> TypeId {
        crate::types::register_type(
            id,
            crate::types::TypeLayout {
                name: format!("heap test type {id}"),
                size,
                ptr_offsets: Vec::new(),
            },
        );
        crate::types::publish();
        id
    }

    #[test]
    fn freeing_twice_is_harmless() {
        let mut heap = Heap::new(false);
        let p = heap.alloc(TYPE_ID_FIRST_USER, 32);
        let q = heap.alloc(TYPE_ID_FIRST_USER, 32);
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
    fn a_reopened_block_is_zeroed() {
        let mut heap = Heap::new(false);
        let size = 1024usize;
        let per_block = BLOCK_BYTES / size;
        // Fill the first block and scribble on every object's body, then
        // start a second block so the first is no longer the open one.
        let first: Vec<*mut u8> = (0..per_block)
            .map(|_| heap.alloc(TYPE_ID_FIRST_USER, size as u32))
            .collect();
        for &p in &first {
            unsafe {
                ptr::write_bytes(
                    p.add(HEADER_SIZE as usize),
                    0xEE,
                    size - HEADER_SIZE as usize,
                )
            };
        }
        let second = heap.alloc(TYPE_ID_FIRST_USER, size as u32);
        assert_ne!(
            (second as usize) >> BLOCK_BITS,
            (first[0] as usize) >> BLOCK_BITS
        );
        // Empty the first block; it goes back to the free list.
        for &p in &first {
            heap.free(p, size);
        }
        // Keep allocating until the first block is reopened.
        let mut reused = None;
        for _ in 0..per_block * 2 {
            let p = heap.alloc(TYPE_ID_FIRST_USER, size as u32);
            if first.contains(&p) {
                reused = Some(p);
                break;
            }
        }
        let p = reused.expect("the emptied block was reused");
        let body = unsafe {
            std::slice::from_raw_parts(p.add(HEADER_SIZE as usize), size - HEADER_SIZE as usize)
        };
        assert!(
            body.iter().all(|&b| b == 0),
            "the reopened block still held old bytes"
        );
        assert!(
            !unsafe { test_flag(p, FLAG_DEAD) },
            "the header was re-stamped"
        );
    }

    #[test]
    fn a_sweep_frees_only_objects_of_the_old_parity() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ty = registered(TYPE_ID_FIRST_USER + 400, 32);
        let mut heap = Heap::new(false);
        let old_a = heap.alloc(ty, 32);
        let old_b = heap.alloc(ty, 32);
        flip_mark_parity();
        let young = heap.alloc(ty, 32);
        assert!(!unsafe { is_marked(old_a) });
        assert!(unsafe { is_marked(young) });

        let is_garbage = |p: *mut u8| !unsafe { is_marked(p) };
        let mut freed = 0;
        for s in 0..heap.spaces.len() {
            for b in 0..SPACE_BLOCKS {
                freed += heap.sweep_block(s, b, &is_garbage);
            }
        }
        // Restore the parity so tests that run afterwards see what they expect.
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
        let mut heap = Heap::new(false);
        let per_block = BLOCK_BYTES / size;
        // One block of objects, then a second block so the first is Full.
        let first: Vec<*mut u8> = (0..per_block)
            .map(|_| heap.alloc(ty, size as u32))
            .collect();
        let _second = heap.alloc(ty, size as u32);
        // Free all but one object in the first block: it is now sparse.
        for &p in &first[1..] {
            heap.free(p, size);
        }
        let survivor = first[0];
        unsafe { (survivor.add(HEADER_SIZE as usize) as *mut u64).write(0x5EED) };
        assert_eq!(heap.select_evacuation((LINES_PER_BLOCK / 4) as u16), 1);
        assert_eq!(heap.stats().live_objects, 2);

        let moved = heap.evacuate(&|_| true);
        assert_eq!(moved, 1);
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
        assert_eq!(heap.stats().live_objects, 2);
        assert_eq!(heap.release_evacuated(), 1);
    }
}
