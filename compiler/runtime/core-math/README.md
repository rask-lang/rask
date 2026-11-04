Correctly rounded binary64 functions from [CORE-MATH](https://core-math.gitlabpages.inria.fr/)
(MIT, `LICENSE` here), upstream commit `85430b3e`, unchanged. Taken from the copy
the `core-math-sys` 1.4.0 crate vendors, one directory per function as upstream
lays them out.

`runtime/math.c` calls them (`cr_sin` and the rest) so every machine and both
backends compute the same bits (`determinism/D12`). Only the functions the stdlib
exposes are here. To add one, copy its directory from upstream's
`src/binary64/<name>/`, keeping the `.c` and the headers it includes.
