/* tslint:disable */
/* eslint-disable */

/**
 * The WASM-facing Ferrite DB session. Holds every Table created in this
 * instance; all operations are synchronous and run on the calling thread.
 */
export class FerriteDb {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Creates a Table with the given name, vector `dimension`, and `metric`
     * (`"cosine"`, `"l2"`, or `"dot"`). Tables carry no metadata columns in
     * this reduced core, so inserted records need no metadata.
     */
    create_table(name: string, dimension: number, metric: string): void;
    /**
     * Creates a Table with a typed Metadata Schema, enabling custom dataset
     * ingestion (FDB-EXP-03). `col_names` and `col_types` must be equal
     * length; `col_types` are `"bool"`, `"i64"`, `"f64"`, or `"string"`.
     */
    create_table_schema(name: string, dimension: number, metric: string, col_names: string[], col_types: string[]): void;
    /**
     * Independent brute-force oracle: scores every vector in `table_name`
     * against `query` using the Table's Metric and returns the `top_k`
     * nearest. Used for exact-match / recall validation distinct from the
     * engine's own search path.
     */
    exact_search(table_name: string, query: Float32Array, top_k: number): SearchHit[];
    /**
     * Appends `vectors.len() / dimension` records to `table_name`. `vectors`
     * is a flat, row-major f32 array aligned with `ids` (one vector per id).
     * Returns the number of vectors now held in the Table.
     */
    insert_records(table_name: string, ids: BigUint64Array, vectors: Float32Array): number;
    /**
     * Appends records with typed metadata to `table_name`. `vectors` is a
     * flat row-major f32 array aligned with `ids`; `values` is a flat array of
     * `ids.len() * col_names.len()` string literals, one per (record, column),
     * parsed according to `col_types`. This is the ingestion path used by
     * custom datasets and the synthetic generator (FDB-EXP-03).
     */
    insert_with_metadata(table_name: string, ids: BigUint64Array, vectors: Float32Array, col_names: string[], col_types: string[], values: string[]): number;
    /**
     * Names of every Table created in this session, in creation order.
     */
    list_tables(): string[];
    /**
     * Creates an empty session.
     */
    constructor();
    /**
     * Exhaustive nearest-neighbour search over `table_name`'s in-memory
     * Deltas, returning the `top_k` nearest hits. Mirrors the native engine's
     * admission-gated exhaustive `search`.
     */
    search(table_name: string, query: Float32Array, top_k: number): SearchHit[];
    /**
     * Whole-session status: how many Tables exist and how many vectors are
     * held in total.
     */
    status(): SessionStatus;
    /**
     * Per-Table status: dimension, Metric, and the in-memory vector count.
     */
    table_status(table_name: string): TableStatus;
}

/**
 * One search result handed back to JS.
 */
export class SearchHit {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Distance of the match under the Table's Metric (lower is nearer).
     */
    distance: number;
    /**
     * The matched vector identifier.
     */
    id: bigint;
}

/**
 * Summary of the whole session.
 */
export class SessionStatus {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Number of Tables created in this session.
     */
    table_count: number;
    /**
     * Total vectors held across every Table.
     */
    vector_count: number;
}

/**
 * Summary of one Table's in-memory state.
 */
export class TableStatus {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Fixed vector dimension.
     */
    readonly dimension: number;
    /**
     * Fixed distance function (`cosine`, `l2`, or `dot`).
     */
    readonly metric: string;
    /**
     * Table name.
     */
    readonly name: string;
    /**
     * Number of vectors currently held in memory.
     */
    readonly vectors: number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_ferritedb_free: (a: number, b: number) => void;
    readonly __wbg_get_searchhit_distance: (a: number) => number;
    readonly __wbg_get_searchhit_id: (a: number) => bigint;
    readonly __wbg_get_sessionstatus_table_count: (a: number) => number;
    readonly __wbg_get_sessionstatus_vector_count: (a: number) => number;
    readonly __wbg_searchhit_free: (a: number, b: number) => void;
    readonly __wbg_sessionstatus_free: (a: number, b: number) => void;
    readonly __wbg_set_searchhit_distance: (a: number, b: number) => void;
    readonly __wbg_set_searchhit_id: (a: number, b: bigint) => void;
    readonly __wbg_set_sessionstatus_table_count: (a: number, b: number) => void;
    readonly __wbg_set_sessionstatus_vector_count: (a: number, b: number) => void;
    readonly __wbg_tablestatus_free: (a: number, b: number) => void;
    readonly ferritedb_create_table: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly ferritedb_create_table_schema: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number];
    readonly ferritedb_exact_search: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly ferritedb_insert_records: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly ferritedb_insert_with_metadata: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number) => [number, number, number];
    readonly ferritedb_list_tables: (a: number) => [number, number];
    readonly ferritedb_new: () => number;
    readonly ferritedb_search: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly ferritedb_status: (a: number) => number;
    readonly ferritedb_table_status: (a: number, b: number, c: number) => [number, number, number];
    readonly tablestatus_dimension: (a: number) => number;
    readonly tablestatus_metric: (a: number) => [number, number];
    readonly tablestatus_name: (a: number) => [number, number];
    readonly tablestatus_vectors: (a: number) => number;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __externref_drop_slice: (a: number, b: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
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
