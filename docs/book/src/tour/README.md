# A tour of Rask

Each step is one short file that teaches one thing. It shows the idea working,
then leaves a function for you to write with only that idea and the ones before
it. The tests at the bottom tell you when you're done:

```bash
rask test 02-functions.rk
```

If you're stuck, `solutions/` has an answer for every step.

## The steps

1. Hello: `main`, `println`
2. Functions: parameters, `return`, `test`
3. Names and strings: `let`, `"{interpolation}"`
4. Names that change: `mut`, and reading a compiler error
5. if and else
6. while
7. for over a range
8. Vec: making one, `push`, `len`, indexing
9. Walking a Vec
10. Structs
11. Methods: `extend`, `self`, static functions
12. Enums and match
13. Enums that carry data

Planned, one idea each, in this order:

14. Optionals: `T?`, `none`, `??`, `? as`
15. Map
16. Errors as values: `T or E`
17. `try` and `catch`
18. Passing values: borrowed by default, `mutate`
19. Moving: `take`, and the error you get using a moved value
20. Copying on purpose: `.clone()`
21. Changing an item in a Vec: `with`
22. Closures
23. Chains: `filter`, `map`, `to_vec`
24. String methods
25. Generic functions
26. Interfaces
27. Cleanup that always runs: `ensure`
28. Racks and links
29. Tasks and channels
30. Shared state
31. Comptime
32. Splitting a program into a package
