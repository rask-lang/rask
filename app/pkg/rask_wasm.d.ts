/* tslint:disable */
/* eslint-disable */

/**
 * Browser-based Rask playground.
 *
 * Provides a simple API for running Rask code and capturing output.
 */
export class Playground {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Check code for errors without running it.
     *
     * Runs the full pipeline (lex → parse → desugar → resolve → typecheck →
     * ownership) and returns JSON diagnostics.
     */
    check(source: string): string;
    /**
     * Create a new playground instance.
     */
    constructor();
    /**
     * Run Rask source code and return output or error.
     *
     * Runs the full compiler pipeline (lex → parse → desugar → resolve →
     * typecheck → ownership) before interpreting, matching `rask run`.
     */
    run(source: string): string;
    /**
     * Get the version of the Rask compiler.
     */
    static version(): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_playground_free: (a: number, b: number) => void;
    readonly playground_check: (a: number, b: number, c: number) => [number, number];
    readonly playground_new: () => number;
    readonly playground_run: (a: number, b: number, c: number) => [number, number, number, number];
    readonly playground_version: () => [number, number];
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
