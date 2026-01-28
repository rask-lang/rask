# Validation Notes: Channel Buffering

## Conflicts with CORE Design?
**NO** — The elaboration preserves all core principles:
- **Transparent costs:** Channel capacity explicit in constructor, blocking visible in function semantics
- **Value semantics:** Channels transfer ownership (existing rule)
- **Local analysis:** Channel types carry no lifetime parameters
- **No annotations:** Channel usage requires no mode annotations

## Internal Consistency?
**YES** — Checked:
- Unbounded never blocks → no SendError::Full variant
- Buffered blocks on full → error only on close
- Rendezvous is buffered(0) semantically
- All error cases enumerated
- Send signature consistent across types

## Conflicts with Other Specs?
**NO** — Cross-checked:
- **Memory model:** Channel send transfers ownership (T1: Send transfers)
- **Error handling:** Result types follow existing pattern
- **Linear types:** Prohibition on Channel<Linear> preserved

## Complete Enough to Implement?
**YES** — Provides:
- Constructor signatures for all 3 types
- Send/recv signatures and semantics
- Blocking behavior for sync vs async
- Error cases enumerated
- Edge cases table

## Concise?
**YES** — Uses tables for edge cases, minimal prose. ~150 lines including examples.

## Notes
- Added OutOfMemory error case for unbounded channels (corner case)
- Specified fairness as implementation-defined (allows optimization)
- Preserved existing affine types and .share() pattern
- try_recv() added for non-blocking use cases (common need)
