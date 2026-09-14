# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Run a shell function over a list of files, one process per core.
#
# Every gate here is the same shape: hundreds of independent `rask` runs, then
# one pass that decides what the results mean. Only the first half can fan out,
# and doing it by hand in each gate is how they drifted — three of them were
# still serial long after the differential harness stopped being.
#
#   fan_out <worker-func> <file>...
#
# The worker gets one file and writes what it found to a file of its own under
# $WORK. Nothing is read from its stdout: parallel workers interleave, so a
# gate that printed as it went would shuffle its own report. The caller reads
# the results back in a fixed order afterwards, which is what keeps output and
# exit code identical to running serially.
#
# The caller exports what the worker reaches — $RASK, $WORK, any helper
# functions. $JOBS sets the width, defaulting to one per core.

fan_out() {
    local worker="$1"; shift
    [ "$#" -eq 0 ] && return 0
    export -f "$worker"
    printf '%s\0' "$@" |
        xargs -0 -r -P "${JOBS:-$(nproc 2>/dev/null || echo 4)}" \
              -I{} bash -c "$worker \"\$@\"" _ {}
}

# A file name safe to use as a result file: the path with slashes flattened.
# Basenames collide across directories — `examples/x.rk` and
# `specs/analysis/prototype/x.rk` would land on the same result.
slot() { echo "${1//\//_}"; }
