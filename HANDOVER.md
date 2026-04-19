# PR2 Handover

Branch: `claude/stdlib-option-auto-wrap-5yiSF` — pushed, 4 commits on top of main.

## What's done

| Commit | What |
|--------|------|
| 5cace1f | Split `ExprKind::Try` → postfix `?` now emits `ExprKind::IsPresent { expr, binding }`; typed as `bool` (OPT10/ER12). Prefix `try x` keeps `ExprKind::Try` |
| c4a02a7 | `if x?` const-narrow on Option/Result (OPT19/ER19/ER21) — narrows scrutinee in then-branch; Result also narrows else to E |
| 2b15366 | `if x? as v` payload binding (OPT20/ER20) — works for mut and anonymous scrutinees |
| c340266 | `!x?` parse error (OPT17/ER26) |

Validated via `rask run --interp` for Option and Result in all three shapes (const, mut, anonymous). Full Rust test suite: only the two pre-existing flaky `c_import_*` failures remain (same on main).

## What's NOT done — queued for PR3 and beyond

From the original 15-item list (session before last):

1. Auto-wrap at assignment for `T?` (OPT6)
2. Reject `Some(x)`/`Ok(x)`/`Err(x)` at construction with migration diagnostic
3. `none` as a true literal (currently aliased to the `None` variant)
4. `else as e` error bind (ER22)
5. `if r is E as e` type-pattern narrow (ER23)
6. `try { … } else |e|` block form (ER17/ER18)
7. Match type patterns for `T or E` (`f64 => …`, `Type as name => …`)
8. Reject match on `T?` with migration diagnostic
9. Disjointness rule (T ≠ E) at type formation
10. `ErrorMessage` bound enforcement
11. Migrate stdlib `.rk` files (t18 etc. still use removed `.is_some()`, `Some()`, `match` on Option)
12. Migrate example `.rk` files

Suggested PR3 scope: items 4, 5, 6 (else-binding family). Items 2, 8, 11, 12 should land together as the migration story. Items 7, 9, 10 are type-system work.

## Landmines the next session must know

1. **Native codegen is broken on Option-returning functions** — pre-existing. Affects `if x is Some` narrowing too. `rask run --interp` works end-to-end; `rask run` (native) segfaults silently; `rask test` uses native codegen and silently reports "0 tests" when Option types appear in the file. Don't chase this during feature work — track separately. Dropped my `tests/suite/t29_presence_narrow.rk` for this reason.

2. **`extract_is_present_narrowing` reads `node_types` set during the cond's `infer_expr`.** It must run AFTER `infer_expr(cond)` in the If handler. Re-inferring would allocate duplicate type vars. See `rask-types/src/checker/check_expr.rs:2058`.

3. **Parser eagerly consumes `as <ident>` after `?` in the postfix handler** (parser.rs:3579–3598) to beat the infix `as` cast operator. Side effect: `x? as i32` outside a condition now binds `i32` rather than casting bool to `i32`. To cast, wrap: `(x?) as i32`. Noted in the commit message but not tested.

4. **Auto-wrap at return for Result with user-defined error types is not fully wired.** My test `return DivError.ByZero` from `func f() -> i32 or DivError` type-errors with "expected i32, found DivError". The auto-wrap machinery landed in PR1 handles Option cleanly and handles Result via the old `Ok`/`Err` wrappers, but bare-variant returns into `T or E` don't wrap. Interacts with item 9 (disjointness) — the branch-by-type logic needs disjoint T and E to work.

5. **Interpreter runtime narrowing is fully inlined into the If handler** (eval_expr.rs ~588). I removed `extract_presence_payload` from dispatch.rs. If PR3 needs `else as e`, it'll want to extend that same block, not reintroduce a helper.

6. **Resolver pushes scope for `as v` bindings in BOTH branches** (resolver.rs ~1981) — intentional, because for Result the else-branch will bind the error (ER21 already narrows the type; ER22 will add the explicit `else as e` form).

## Rask syntax reminder (from CLAUDE.md)

Use Rask, not Rust — `const`/`let`, `func`, `extend`, `public`, `string`, explicit `return`, `T or E` not `Result<T,E>`, `T?` not `Option<T>`.

## Commands

- Build: `cd compiler && cargo build --release -p rask-cli`
- Binary: `compiler/target/release/rask`
- Typecheck: `rask check <file>`
- Run (prefer this for Option/Result): `rask run --interp <file>`
- Tests: `cargo test --workspace --release` (ignore the two `c_import_*` flakes)

## Spec references

- `specs/types/optionals.md` — OPT1–OPT30
- `specs/types/error-types.md` — ER1–ER43
- `specs/types/error-model-redesign-proposal.md` — decision record
