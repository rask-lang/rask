// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a container's elements are, and which calls build one.
//!
//! A container is a byte store. It knows how big an element is and nothing
//! else, so it can't tell a sixteen-byte string from a sixteen-byte struct —
//! and `free` has to know, or the strings inside never come back (#1027).
//!
//! The answer is settled once, where a container is constructed, by the only
//! place that has it: lowering, reading the checker's type. It travels as the
//! element's own type (`MirConst::Elem`), with its containers still named;
//! codegen turns it into the offsets of what one element owns, and the runtime
//! keeps that list on the container itself. Nothing downstream re-derives it —
//! not the drop pass, not the caller of a function that hands a container
//! back, not an inlined copy.
//!
//! It used to travel as one `i64`: small numbers for a string or a container,
//! a range for struct layouts, another for enums, a flag bit for wrappers. A
//! tuple fit nowhere in that, so `Vec<(string, i64)>` described its elements
//! as owning nothing and every string in it leaked (#1395).

use crate::{MirConst, MirOperand, MirType};

/// The constructor argument describing elements of type `ty`. Pass the type
/// with its containers named (`payload_to_mir`), or a `Vec<Vec<i64>>` frees
/// none of its inner vectors.
pub fn elem(ty: MirType) -> MirOperand {
    MirOperand::Constant(MirConst::Elem(ty))
}

/// What a box holds, as the number the runtime stores on it.
///
/// A box owns its payload: `Shared.mutex(Map.new())` moves the map in, and the
/// map's free has to happen when the box's last reference goes — which only
/// the runtime knows, because only it counts them. So the kind travels to the
/// constructor and lives on the box, the same way a container's element
/// descriptor does. Without it `Shared<Map<string, i64>, Mutex>` freed the
/// mutex and left the map and its tables behind.
///
/// An arena is not a box's payload, and a string is refcounted and released
/// by whoever put it in.
///
/// These values are duplicated in `rask_runtime.h` as `RASK_BOX_PAYLOAD_*`.
/// They are a handful of integers with no other reader; keeping them in step
/// is a comment because generating them would be more machinery than the
/// thing itself.
pub const BOX_PAYLOAD_NONE: i64 = 0;
pub const BOX_PAYLOAD_VEC: i64 = 1;
pub const BOX_PAYLOAD_MAP: i64 = 2;
/// A closure block. Same reason as the two byte stores: the box holds the
/// address and the frame that built the closure gave it away, so the block
/// comes back when the box's last reference does (#1253).
pub const BOX_PAYLOAD_CLOSURE: i64 = 3;
/// Another box. Its strategy isn't this box's to know: every box starts with
/// its own release, and the runtime calls that (#1302).
pub const BOX_PAYLOAD_BOX: i64 = 4;

/// The payload kind for a checker type and its head name.
///
/// A function type has no head name, so asking by name alone could only ever say
/// `NONE`, and every `Shared.local(|x| …)` leaked the closure.
pub fn box_payload_kind_of(ty: &rask_types::Type, head: Option<&str>) -> i64 {
    if matches!(ty, rask_types::Type::Fn { .. }) {
        return BOX_PAYLOAD_CLOSURE;
    }
    match head {
        Some("Vec") => BOX_PAYLOAD_VEC,
        Some("Map") => BOX_PAYLOAD_MAP,
        Some("Shared") => BOX_PAYLOAD_BOX,
        _ => BOX_PAYLOAD_NONE,
    }
}

/// Which `Map` constructor a key type wants.
///
/// A string key hashes and compares by its contents. A *link* key compares by
/// its word — two links are equal when they name the same node — but hashing
/// that word hashes the address the allocator handed out, which moves with
/// whatever the program allocated before the rack was built. Sim replays the
/// map seed so a replay walks the buckets in the same order (determinism/D7),
/// and an address in the hash input is the one thing that doesn't replay, so a
/// link key buckets by its node's slot instead (#1268). Anything else buckets
/// by its word.
///
/// One place, because three call sites used to spell the string case by hand
/// and a fourth kind would have had to find all of them.
pub fn map_ctor_for(key_ty: &MirType) -> &'static str {
    match key_ty {
        MirType::String => "Map_new_string_keys",
        MirType::Link(_) => "Map_new_link_keys",
        MirType::Option(inner) if matches!(**inner, MirType::Link(_)) => "Map_new_link_keys",
        _ => "Map_new",
    }
}

/// The same decision for `Map.with_capacity(n)`, which takes the capacity
/// between the sizes and the tags but buckets by exactly the same rule.
///
/// Derived from `map_ctor_for` rather than repeating the match, so a fourth key
/// kind is added in one place and this follows.
pub fn map_ctor_with_capacity(key_ty: &MirType) -> &'static str {
    match map_ctor_for(key_ty) {
        "Map_new_string_keys" => "Map_with_capacity_string_keys",
        "Map_new_link_keys" => "Map_with_capacity_link_keys",
        _ => "Map_with_capacity",
    }
}

/// Every call that hands back a container the caller owns: how many size
/// arguments come first, how many element descriptions follow them, and what frees the
/// result.
///
/// One list, read by everything that needs it: lowering appends that many descriptions,
/// codegen's dispatch table builds the C signature from it and expands the descriptions
/// into offset pointers, the pre-pass that registers those offset blobs finds
/// the descriptions with it, and the drop pass knows a fresh container when it sees one
/// come out of a call to one of these.
///
/// The free function is spelled out rather than guessed from the name, because
/// the two part company: `Map_keys` is a `Map_` call that hands back a `Vec`,
/// and freeing that with `Map_free` would read a Vec as a hash table.
pub const CTORS: &[(&str, u8, u8, &str)] = &[
    ("Vec_new", 1, 1, "Vec_free"),
    // `mut v: Vec<T> = []` and `Vec.from([...])`: the elements come from a
    // static blob, but anything pushed later does not.
    ("rask_vec_from_static", 3, 1, "Vec_free"),
    // An array receiver seen as a Vec. The array keeps its elements, so the
    // free gives back the copy and nothing in it (#1405).
    ("rask_vec_view", 3, 1, "Vec_free_view"),
    ("Vec_with_capacity", 2, 1, "Vec_free"),
    ("Vec_fixed", 2, 1, "Vec_free"),
    // `skip`/`take` outside a fused chain call the runtime, which hands back a
    // freshly allocated Vec. No size arguments and no element descriptions — the source
    // Vec already carries both, and the runtime copies them across — so these
    // are here only to tell the drop pass the result is the caller's to free.
    ("Vec_skip", 0, 0, "Vec_free"),
    ("Vec_take", 0, 0, "Vec_free"),
    // `chunks` hands back a fresh `Vec<Vec<T>>`, and the runtime builds it
    // with an element map saying the elements are Vec handles — so freeing it
    // frees the chunks too. The zero here is this table's own count, which
    // describes what *lowering* knows; the nested answer is settled at the
    // construction site in `rask_vec_chunks`.
    ("Vec_chunks", 0, 0, "Vec_free"),
    ("Map_new", 2, 2, "Map_free"),
    ("Map_new_string_keys", 2, 2, "Map_free"),
    ("Map_with_capacity", 3, 2, "Map_free"),
    ("Map_with_capacity_string_keys", 3, 2, "Map_free"),
    ("Map_with_capacity_link_keys", 3, 2, "Map_free"),
    ("Map_new_link_keys", 2, 2, "Map_free"),
    ("Map_new_keyed", 4, 2, "Map_free"),
    ("Map_with_capacity_keyed", 5, 2, "Map_free"),
    // `keys`, `values` and `entries` walk a map and hand back a fresh Vec of
    // what they found — a `Map_` name with a `Vec` result, which is why the
    // free is written down rather than read off the prefix.
    ("Map_keys", 0, 0, "Vec_free"),
    ("Map_values", 0, 0, "Vec_free"),
    ("Map_entries", 0, 0, "Vec_free"),
    // Racks and pools carry no element description: a rack is told about its fields
    // separately, through `Link_register_*`, and a pool's slots are opaque
    // bytes. They are here so the drop pass recognises one coming out of a
    // constructor — `rask_rack_free` and `rask_pool_free` have existed all
    // along with nothing calling them, so `Rack.new()` with nothing in it
    // leaked (#1048).
    // A `Random` is a heap block behind an opaque handle — no elements, no
    // sizes, and nothing was freeing it. Here so the drop pass knows the
    // caller owns what came back.
    ("Random_new", 0, 0, "Random_free"),
    ("Random_from_seed", 0, 0, "Random_free"),
    // An `Atomic<T>` is the same shape: one heap word behind an opaque handle,
    // with a free that didn't exist. Every counter in a program leaked eight
    // bytes, and `Atomic<T>` is what mem.atomics/GA1 makes you write.
    ("Atomic_new", 0, 0, "Atomic_free"),
    ("Atomic_default", 0, 0, "Atomic_free"),
    ("Rack_new", 0, 0, "Rack_free"),
    ("Rack_snapshot", 1, 0, "Rack_free"),
    // `rack.nodes()` walks the directory and pushes each node's address into a
    // fresh Vec. The elements are links, which own nothing — freeing the
    // vector doesn't touch a node. `for n in s.nodes()` leaked one vector per
    // call, which is most of what the snapshot files were carrying.
    ("Rack_nodes", 0, 0, "Vec_free"),
    // Clone the elements into a new vector and clear the source, so the result
    // owns them and carries the source's element map. The source keeps its own
    // allocation, empty.
    ("Vec_take_all", 0, 0, "Vec_free"),
    // Not `fs.read_lines`, `fs.read_bytes` or `File.lines`, even though the
    // runtime has a function for each: those three are written in Rask now, so
    // these names reach MIR as ordinary functions with bodies and the pass
    // works out the answer itself — including that each hands its vector back
    // *inside* a `Vec<T> or IoError`, which a line here can't say. Listing
    // them overrode that with "a bare Vec" and the caller freed the wrapper as
    // one: `fs.read_lines(p) catch _ => Vec.new()` tripped the borrow guard.
    //
    // `chars()` yields scalars and `graphemes()` copies each cluster into a
    // fresh string. Neither points into the source — unlike the splitters
    // below, which look identical from here and are not.
    ("string_chars", 0, 0, "Vec_free"),
    ("string_graphemes", 0, 0, "Vec_free"),
    // The third of that family and the one left out: `char_indices` pushes a
    // (byte offset, scalar) pair per character into a fresh vector. Two
    // integers — nothing points into the source, and nothing in it owns
    // anything — so the vector is the caller's, elements and all.
    ("string_char_indices", 0, 0, "Vec_free"),
    // The three the runtime builds from the OS: each copies what it found into
    // fresh strings and carries the element map, so the vector it hands back is
    // the caller's to free — elements and all. They were the largest single
    // leak left in the suite once the closures were fixed: 150 strings for one
    // `os.env_vars()`, and `t_os_env.rk` and `t41_os.rk` between them held 304.
    //
    // Unlike the string splitters below, nothing here is a view into a source
    // the caller still holds.
    ("os_env_vars", 0, 0, "Vec_free"),
    ("os_args", 0, 0, "Vec_free"),
    // `cli.args()` is the same argv as `os.args()`, built the same way, and
    // was left off: every program that read its arguments leaked them.
    ("cli_args", 0, 0, "Vec_free"),
    // A `Shared` box carries no element description — its payload is opaque bytes it
    // was handed, the same as a pool slot. It is here for the same reason
    // `Rack_new` is: `rask_shared_free` has existed all along with nothing
    // calling it, so `Shared.new(0)` and nothing else leaked the box and its
    // payload — two allocations, whatever the payload was (#1099).
    //
    // The free is a release, not a free: `Shared_drop` decrements and frees at
    // zero, and `Shared_clone` is what incremented. So a box handed to a task
    // outlives the frame that made it, which is the point of the type.
    ("Shared_new", 0, 0, "Shared_drop"),
    // The two halves of a channel. `let (tx, rx) = Channel<T>.buffered(n)`
    // reaches MIR as the constructor plus one accessor per half, and the
    // accessor's result is the handle — one sender, one receiver, which is
    // what the channel's counts are initialised to. Dropping a handle closes
    // that end, and the channel and its buffer go when both ends are gone;
    // `rask_sender_drop` and `rask_recver_drop` have done all of that since
    // channels were written, with nothing calling them. Seven suite files were
    // carrying it — `t_select.rk` 74 allocations, fifteen channels' worth.
    //
    // The constructor itself is deliberately absent: what it hands back is the
    // channel, which the two drops own between them.
    ("channel_tx", 0, 0, "Sender_drop"),
    ("channel_rx", 0, 0, "Receiver_drop"),
    ("Sender_clone", 0, 0, "Sender_drop"),
    ("string_split", 0, 0, "Vec_free"),
    ("string_lines", 0, 0, "Vec_free"),
    ("string_split_whitespace", 0, 0, "Vec_free"),
    // A string builder is the frame's until `build()` takes it away. Nothing
    // released one on a path that gives up before building, and
    // `string.from_utf8` returns a `Utf8Error` from eight places — so every
    // rejected byte sequence leaked the builder and its buffer.
    ("StringBuilder_new", 0, 0, "StringBuilder_free"),
    ("StringBuilder_with_capacity", 0, 0, "StringBuilder_free"),
    // A cstring owns the NUL-terminated copy it made, and it is the caller's to
    // free — that is what makes it different from `string.as_ptr()`, which
    // points into a buffer the string still holds (#949).
    ("string_copy_terminated", 0, 0, "cstring_free"),
    // The other two strategies build their own runtime object, so each needs
    // its own release: `Shared.mutex(0)` is `Mutex_new` and `Shared.local(0)`
    // is `Cell_new`, and neither is `Shared_new`. Only the third was listed, so
    // two of the three constructors leaked the box and its payload — the same
    // two allocations #1099 measured, for the two spellings it didn't test.
    ("Mutex_new", 0, 0, "Mutex_drop"),
    ("Cell_new", 0, 0, "Cell_drop"),
    // The clones. These were absent because `clone_elision` can decide a clone
    // is unnecessary and leave the caller's own container in the slot, and
    // freeing that is a double free — `return v.clone()` printed the right
    // length and died on the way out. The drop pass now runs *after* elision,
    // so what it sees is what runs: an elided clone is an assignment, not a
    // call, and never looks like a fresh container at all (#1050, #1045).
    ("Vec_clone", 0, 0, "Vec_free"),
    ("Map_clone", 0, 0, "Map_free"),
    // `bytes()` builds a fresh Vec of the string's bytes and hands it over —
    // no view into the source, so nothing reads it after the frame ends. The
    // splitters below are the family it belongs to and stay out for the reason
    // written there; this one was measured on its own.
    ("string_bytes", 0, 0, "Vec_free"),
    // Same shape on the C side: the bytes up to the terminator, copied into a
    // fresh Vec that `string.from_utf8` reads and nobody else holds (#949).
    ("cstring_bytes", 0, 0, "Vec_free"),
    // Bytes off standard input, into a Vec the runtime made for this call and
    // nothing else holds. Without it `Stdin.read_bytes` handed back a vector
    // nobody owned — and, because an interface call's answer is only as good as the
    // worst implementation behind it, it also stopped `reader.read_bytes()`
    // from being anyone's (#1199).
    ("io_read_std_bytes", 0, 0, "Vec_free"),
    //
    // The string splitters — `string_split`, `string_lines` and friends — are
    // absent for a nearer reason: each does hand back a fresh Vec, and
    // registering them clears the leak, but `simple_grep` then finds nothing
    // and `markdown_renderer` aborts on `malloc(): unaligned tcache chunk`.
    // Their result is a Vec of *views into the source string*, so the elements
    // outlive the free. `bytes()` copies instead, which is why it is listed
    // above and they are not. Measured both ways on #1050.
];

/// `(leading sizes, element descriptions)` for a container constructor, by the name MIR
/// calls it — monomorphization's `$` suffix and any module path stripped.
pub fn ctor_shape(name: &str) -> Option<(usize, usize)> {
    entry(name).map(|(_, l, t, _)| (*l as usize, *t as usize))
}

/// What frees the container this call handed back, or `None` if it isn't one.
pub fn free_fn(name: &str) -> Option<&'static str> {
    entry(name).map(|(_, _, _, free)| *free)
}

/// Natives that hand back a fresh container *inside* a wrapper — `Vec<u8>?`,
/// `T or E` — with the free that matches it. The caller owns what it
/// unwraps; the wrapper itself is a value.
///
/// `CTORS` can't say this: its entries mean the call's result *is* the
/// container, and registering one of these there freed the wrapper as if it
/// were the vector. A Rask function returning a wrapped container gets the
/// same answer from the hand-back analysis in container_drop; this is that
/// answer for a native, which has no body to analyse.
pub const WRAPPED_CTORS: &[(&str, &str)] = &[
    // Bytes off a socket, in a Vec the runtime made for this call; `none`
    // when the read failed. `TcpConnection.read_bytes` is one of the bodies
    // behind `reader.read_bytes()`, and an interface call's result is only owned
    // when every body hands back a fresh container — so leaving this out
    // made every reader's bytes nobody's, a `Buffer`'s included.
    ("TcpConnection_read_bytes_raw", "Vec_free"),
    // A file's bytes from the current position, the same shape and for the
    // same reason: `File.read_bytes` is another body behind `reader.read_bytes()`.
    ("File_read_bytes_raw", "Vec_free"),
];

/// What frees the container inside the wrapper this native call handed back.
pub fn wrapped_free_fn(name: &str) -> Option<&'static str> {
    let head = name.rsplit("::").next().unwrap_or(name);
    let base = head.split('$').next().unwrap_or(head);
    WRAPPED_CTORS.iter().find(|(n, _)| *n == base).map(|(_, free)| *free)
}

fn entry(name: &str) -> Option<&'static (&'static str, u8, u8, &'static str)> {
    let head = name.rsplit("::").next().unwrap_or(name);
    let base = head.split('$').next().unwrap_or(head);
    CTORS.iter().find(|(n, _, _, _)| *n == base)
}
