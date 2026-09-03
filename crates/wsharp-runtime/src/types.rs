//! The runtime type registry.
//!
//! The code generator registers one [`TypeLayout`] per heap type when it sets
//! the JIT up. One table, two consumers: the collector reads `ptr_offsets` to
//! trace an object's outgoing references, and multiple dispatch reads a type id
//! out of an object header to name the type it belongs to.
//!
//! Registration happens once, before any code runs, and the table never changes
//! afterwards. That is worth exploiting, because tracing looks a type up *per
//! object visited*: [`publish`] freezes the map into a flat array indexed by
//! type id, which [`info`] then reads with no lock and no allocation. The
//! `HashMap` behind `register_type` exists only for the build phase.

use std::collections::HashMap;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::header::{AUX_OFFSET, HEADER_SIZE, TYPE_ID_STR, TypeId, align_up};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeLayout {
    pub name: String,
    /// Total object size in bytes, header included. Zero means the size is not
    /// fixed and must be read from the object -- strings, and later arrays,
    /// keep their length in the `aux` word.
    pub size: u32,
    /// Byte offsets, from the start of the object, of every field holding a
    /// pointer to another heap object. This is what makes tracing possible.
    pub ptr_offsets: Vec<u32>,
}

/// A published layout: the same information, but borrowed for the process's
/// lifetime so the tracer can read it without copying.
#[derive(Debug)]
pub struct TypeInfo {
    pub name: &'static str,
    pub size: u32,
    pub ptr_offsets: &'static [u32],
}

impl TypeInfo {
    /// Whether instances carry their own size in the `aux` word.
    pub fn has_variable_size(&self) -> bool {
        self.size == 0
    }
}

#[derive(Default)]
struct Registry {
    layouts: HashMap<TypeId, TypeLayout>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// The published table, or null before the first [`publish`].
///
/// Deliberately leaked. It must outlive every JIT-compiled frame that might
/// read it, and those live until the process exits -- `JITModule` is never
/// dropped for exactly the same reason. Publishing again leaks the previous
/// table rather than freeing it, because a collector thread may be reading it.
static PUBLISHED: AtomicPtr<Vec<Option<TypeInfo>>> = AtomicPtr::new(ptr::null_mut());

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Registry::default))
}

pub fn register_type(id: TypeId, layout: TypeLayout) {
    with_registry(|r| r.layouts.insert(id, layout));
}

/// Freeze the registry into the flat table [`info`] reads.
///
/// Call once code generation has registered every type and before any
/// generated code runs.
pub fn publish() {
    // Strings are heap objects too, and the collector will meet them. They have
    // no outgoing references and no fixed size: the length is in `aux`.
    register_type(
        TYPE_ID_STR,
        TypeLayout {
            name: "str".into(),
            size: 0,
            ptr_offsets: Vec::new(),
        },
    );

    let table = with_registry(|r| {
        let top = r.layouts.keys().copied().max().unwrap_or(0) as usize;
        let mut table: Vec<Option<TypeInfo>> = (0..=top).map(|_| None).collect();
        for (id, layout) in &r.layouts {
            table[*id as usize] = Some(TypeInfo {
                name: Box::leak(layout.name.clone().into_boxed_str()),
                size: layout.size,
                ptr_offsets: Box::leak(layout.ptr_offsets.clone().into_boxed_slice()),
            });
        }
        table
    });
    PUBLISHED.store(Box::into_raw(Box::new(table)), Ordering::Release);
}

/// The layout of `id`, with no lock and no allocation.
///
/// This is the tracing hot path: one atomic load and one bounds-checked index.
pub fn info(id: TypeId) -> Option<&'static TypeInfo> {
    let table = PUBLISHED.load(Ordering::Acquire);
    if table.is_null() {
        return None;
    }
    // Sound because the table is leaked and never mutated after publication,
    // so the reference really does live as long as the process.
    let table: &'static Vec<Option<TypeInfo>> = unsafe { &*table };
    table.get(id as usize)?.as_ref()
}

/// The size in bytes of the object at `ptr`, header included.
///
/// # Safety
/// `ptr` must point at a live, un-forwarded W# heap object whose type has been
/// published.
pub unsafe fn object_size(ptr: *const u8) -> Option<u32> {
    let info = info(unsafe { crate::header::type_id_of(ptr) })?;
    if !info.has_variable_size() {
        return Some(info.size);
    }
    // Variable-sized: the element count is in `aux`, right after the header.
    let aux = unsafe { (ptr.offset(AUX_OFFSET as isize) as *const u64).read() };
    Some(align_up(HEADER_SIZE + aux as u32))
}

pub fn layout_of(id: TypeId) -> Option<TypeLayout> {
    with_registry(|r| r.layouts.get(&id).cloned())
}

pub fn type_name(id: TypeId) -> Option<String> {
    with_registry(|r| r.layouts.get(&id).map(|l| l.name.clone()))
}

pub fn registered_type_count() -> usize {
    with_registry(|r| r.layouts.len())
}

#[doc(hidden)]
pub fn reset_types_for_tests() {
    with_registry(|r| r.layouts.clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::TYPE_ID_FIRST_USER;

    #[test]
    fn registered_layouts_can_be_read_back() {
        let id = TYPE_ID_FIRST_USER + 100;
        register_type(
            id,
            TypeLayout {
                name: "Pair".into(),
                size: 32,
                ptr_offsets: vec![16, 24],
            },
        );
        let layout = layout_of(id).expect("layout was registered");
        assert_eq!(layout.name, "Pair");
        assert_eq!(layout.size, 32);
        assert_eq!(layout.ptr_offsets, vec![16, 24]);
        assert_eq!(type_name(id).as_deref(), Some("Pair"));
    }

    #[test]
    fn unknown_type_ids_have_no_layout() {
        assert!(layout_of(TYPE_ID_FIRST_USER + 999).is_none());
    }

    #[test]
    fn publishing_makes_layouts_readable_without_a_lock() {
        let id = TYPE_ID_FIRST_USER + 200;
        register_type(
            id,
            TypeLayout {
                name: "Node".into(),
                size: 24,
                ptr_offsets: vec![16],
            },
        );
        publish();

        let node = info(id).expect("published");
        assert_eq!(node.name, "Node");
        assert_eq!(node.size, 24);
        assert_eq!(node.ptr_offsets, &[16]);
        assert!(!node.has_variable_size());

        // Past the end of the table, and a gap inside it, both read as absent
        // rather than panicking -- the tracer meets ids it has never seen.
        assert!(info(TYPE_ID_FIRST_USER + 9999).is_none());
        assert!(info(TYPE_ID_FIRST_USER + 199).is_none());
    }

    #[test]
    fn strings_report_the_size_recorded_in_their_aux_word() {
        publish();
        let str_info = info(TYPE_ID_STR).expect("publish registers `str`");
        assert!(str_info.has_variable_size());
        assert!(
            str_info.ptr_offsets.is_empty(),
            "a string holds no references"
        );

        // Lay out a string exactly as the code generator emits one.
        // The fields exist to be laid out in memory, not read through the
        // Rust type: `object_size` reads them back through a raw pointer.
        #[repr(align(16))]
        #[allow(dead_code)]
        struct Str {
            meta: u64,
            aux: u64,
            bytes: [u8; 8],
        }
        let s = Str {
            meta: crate::header::meta_word(TYPE_ID_STR, crate::header::FLAG_IMMORTAL),
            aux: 5,
            bytes: *b"hello\0\0\0",
        };
        let size = unsafe { object_size(&s as *const Str as *const u8) };
        assert_eq!(size, Some(align_up(HEADER_SIZE + 5)));
    }
}
