# Finding a leak

`RASK_LEAK_CHECK=1` says how many allocations a program still holds at exit.
`RASK_LEAK_TRACE=1` says where they came from — the runtime function that
allocated each survivor, grouped and counted:

```
$ RASK_LEAK_CHECK=1 RASK_LEAK_TRACE=1 ./prog
rask: 17 allocations never released (448 bytes, undercounted)
  still held, by the runtime function that allocated it:
    17 allocations, 448 bytes — 0x40f89e (addr2line -fe <binary> 0x40f89e)
```

The address is `__builtin_return_address(0)` inside the allocator, so it lands
in `rask_vec_new`, `rask_closure_alloc`, `rask_string_from_parts` — whichever
one asked. A static link exports no symbols, so resolve it yourself:

```
$ addr2line -fe ./prog 0x40f89e
rask_closure_alloc
```

For a test file, keep the binary the harness builds:

```
$ RASK_KEEP_TEST_BIN=1 RASK_LEAK_CHECK=1 RASK_LEAK_TRACE=1 \
    rask test tests/suite/p21_sequence_adapters.rk
warning: test binary kept at /tmp/rask_test_10868
...
$ addr2line -fe /tmp/rask_test_10868 0x40f89e
```

Then narrow it to one test with `-f <pattern>`: the counts are per run, so
bisecting by name is quick.

That is how the two closure leaks in `#1045`'s tail were separated — one was
the environment holding a container, the other was a closure passed as an
argument that nobody dropped. Both looked identical in the count.

`tests/known_leaks.txt` carries a count per file; `tests/leak_gate.sh` enforces
"no new leaks" and names any file that has stopped leaking.
