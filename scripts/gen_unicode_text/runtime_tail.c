// ─── Lookups ───────────────────────────────────────────────────────────

static int rask_in_ranges(uint32_t c, const RaskCharRange *r, size_t n) {
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (c < r[mid].lo) hi = mid;
        else if (c > r[mid].hi) lo = mid + 1;
        else return 1;
    }
    return 0;
}

// Columns this scalar occupies. Control characters are zero — emitting them is
// the caller's problem, not the width's.
int rask_scalar_width(uint32_t c) {
    if (c == 0) return 0;
    if (c < 0x20 || (c >= 0x7F && c < 0xA0)) return 0;
    if (c < 0x7F) return 1;
    if (rask_in_ranges(c, RASK_ZERO_WIDTH, RASK_ZERO_WIDTH_LEN)) return 0;
    if (rask_in_ranges(c, RASK_WIDE, RASK_WIDE_LEN)) return 2;
    return 1;
}

int rask_grapheme_joins_left(uint32_t c) {
    return rask_in_ranges(c, RASK_GRAPHEME_JOIN_LEFT, RASK_GRAPHEME_JOIN_LEFT_LEN);
}

int rask_grapheme_is_prepend(uint32_t c) {
    return rask_in_ranges(c, RASK_GRAPHEME_PREPEND, RASK_GRAPHEME_PREPEND_LEN);
}

uint8_t rask_ccc(uint32_t c) {
    size_t lo = 0, hi = RASK_CCC_LEN;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (c < RASK_CCC[mid].cp) hi = mid;
        else if (c > RASK_CCC[mid].cp) lo = mid + 1;
        else return RASK_CCC[mid].ccc;
    }
    return 0;
}

// Canonical decomposition of one scalar, or 0 when it has none. Hangul is
// algorithmic (UAX #15) rather than tabulated.
int rask_canonical_decompose(uint32_t c, uint32_t *out, int cap) {
    if (c >= 0xAC00 && c <= 0xD7A3) {
        uint32_t s = c - 0xAC00;
        uint32_t l = 0x1100 + s / (21 * 28);
        uint32_t v = 0x1161 + (s % (21 * 28)) / 28;
        uint32_t t = s % 28;
        if (cap < 3) return 0;
        out[0] = l; out[1] = v;
        if (t == 0) return 2;
        out[2] = 0x11A7 + t;
        return 3;
    }
    size_t lo = 0, hi = RASK_DECOMP_LEN;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (c < RASK_DECOMP[mid].cp) hi = mid;
        else if (c > RASK_DECOMP[mid].cp) lo = mid + 1;
        else {
            uint32_t n = RASK_DECOMP[mid].len;
            if ((int)n > cap) return 0;
            memcpy(out, &RASK_DECOMP_POOL[RASK_DECOMP[mid].off], n * sizeof(uint32_t));
            return (int)n;
        }
    }
    return 0;
}

// The primary composite of a+b, or 0 when the pair doesn't compose. Composition
// exclusions are already absent from the table.
uint32_t rask_canonical_compose(uint32_t a, uint32_t b) {
    // Hangul, again algorithmic.
    if (a >= 0x1100 && a < 0x1113 && b >= 0x1161 && b < 0x1176) {
        return 0xAC00 + ((a - 0x1100) * 21 + (b - 0x1161)) * 28;
    }
    if (a >= 0xAC00 && a <= 0xD7A3 && (a - 0xAC00) % 28 == 0
        && b > 0x11A7 && b < 0x11C3) {
        return a + (b - 0x11A7);
    }
    size_t lo = 0, hi = RASK_COMPOSE_LEN;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        const RaskCompose *e = &RASK_COMPOSE[mid];
        if (a < e->a || (a == e->a && b < e->b)) hi = mid;
        else if (a > e->a || (a == e->a && b > e->b)) lo = mid + 1;
        else return e->c;
    }
    return 0;
}
