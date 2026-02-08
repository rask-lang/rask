# Design Process

Rask's design is guided by clear principles and measured against specific metrics.

## Design Principles

1. **Safety Without Annotation** - Memory safety without lifetime markers
2. **Value Semantics** - No hidden sharing or aliasing
3. **No Storable References** - References can't escape scope
4. **Transparent Costs** - Major costs visible in code
5. **Local Analysis Only** - No whole-program inference
6. **Resource Types** - I/O handles must be consumed
7. **Compiler Knowledge is Visible** - IDE shows inferred information

Full details: [CORE_DESIGN.md](https://github.com/rask-lang/rask/blob/main/CORE_DESIGN.md)

## Validation

Rask is validated against test programs that must work naturally:

1. HTTP JSON API server
2. grep clone ✓ (implemented)
3. Text editor with undo ✓ (implemented)
4. Game loop with entities ✓ (implemented)
5. Embedded sensor processor

**Litmus test:** If Rask is longer/noisier than Go for core loops, fix the design.

## Metrics

Design decisions are evaluated using concrete metrics:
- Clone overhead (% of lines with .clone())
- Handle access cost (nanoseconds)
- Compile times (seconds per 1000 LOC)
- Binary size
- Memory usage

See [METRICS.md](https://github.com/rask-lang/rask/blob/main/specs/METRICS.md) for the scoring methodology.

## Specs and RFCs

Language features are documented as formal specifications in [specs/](https://github.com/rask-lang/rask/tree/main/specs).

Major changes follow an RFC process:
1. Open an issue for discussion
2. Draft a specification
3. Implement in interpreter
4. Validate against litmus tests
5. Update metrics
6. Merge if it improves the design

## Tradeoffs

Every design has tradeoffs. Rask makes these intentional choices:

- **More `.clone()` calls** - Better than lifetime annotations (our view)
- **Handle overhead** - Better than raw pointers with manual tracking
- **No storable references** - Simpler mental model, requires restructuring some patterns
- **Explicit costs** - Better than hidden complexity

See [CORE_DESIGN.md § Tradeoffs](https://github.com/rask-lang/rask/blob/main/CORE_DESIGN.md#design-tradeoffs) for full discussion.

## Contributing to Design

When proposing changes:

1. **Explain the problem** - What use case is difficult today?
2. **Show the tradeoff** - What does this cost?
3. **Test against litmus tests** - Does it make real programs better or worse?
4. **Measure the impact** - Update relevant metrics
5. **Consider alternatives** - What other approaches exist?

The goal is ergonomics without hidden costs. If a feature hides complexity or breaks transparency, it probably doesn't belong.

## Philosophy

> "Safety is a property, not an experience."

Users shouldn't think about memory safety—they should just write code. The type system and scope rules make unsafe operations impossible by construction.

> "If Rask needs 3+ lines where Go needs 1, question the design."

Ceremony should be minimal. Explicit costs are good; boilerplate is bad.

> "Local analysis only."

Compilation should scale linearly. No whole-program inference, no escape analysis. Function signatures tell the whole story.

## Learn More

- [CORE_DESIGN.md](https://github.com/rask-lang/rask/blob/main/specs/CORE_DESIGN.md) - Complete design rationale
- [METRICS.md](https://github.com/rask-lang/rask/blob/main/specs/METRICS.md) - How we measure success
- [TODO.md](https://github.com/rask-lang/rask/blob/main/TODO.md) - What's being worked on
- [Formal Specifications](../reference/specs-link.md) - Detailed technical specs
