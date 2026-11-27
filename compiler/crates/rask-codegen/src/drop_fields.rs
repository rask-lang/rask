// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What frees a field, by the field's declared type: the walk in `builder.rs`
//! that releases a value dying in a frame asks here which free a container or
//! handle field takes. A value dying inside a box that owns it is released by
//! the vtable's `owned_release`, which walks the same description a container
//! element gets (`elem_offsets::owned_offsets`).

pub use rask_mono::drop_names::{box_release_for, container_free_for, HandleReleases};
