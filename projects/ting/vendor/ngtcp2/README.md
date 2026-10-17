ngtcp2 1.25.0's core library (`lib/`), unmodified, from the source vendored in the
`ngtcp2-sys` 0.2.0 crate. MIT licensed, see `COPYING`.

Only the core: ngtcp2's crypto backends are left out, since projects/ting
supplies its own crypto callbacks. `version.h` is `version.h.in` filled in, and
`config.h` holds the Linux answers CMake would have detected.
