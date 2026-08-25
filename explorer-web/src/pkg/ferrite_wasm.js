/* @ts-self-types="./ferrite_wasm.d.ts" */

/**
 * The WASM-facing Ferrite DB session. Holds every Table created in this
 * instance; all operations are synchronous and run on the calling thread.
 */
export class FerriteDb {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        FerriteDbFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_ferritedb_free(ptr, 0);
    }
    /**
     * Creates a Table with the given name, vector `dimension`, and `metric`
     * (`"cosine"`, `"l2"`, or `"dot"`). Tables carry no metadata columns in
     * this reduced core, so inserted records need no metadata.
     * @param {string} name
     * @param {number} dimension
     * @param {string} metric
     */
    create_table(name, dimension, metric) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(metric, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_create_table(this.__wbg_ptr, ptr0, len0, dimension, ptr1, len1);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Creates a Table with a typed Metadata Schema, enabling custom dataset
     * ingestion (FDB-EXP-03). `col_names` and `col_types` must be equal
     * length; `col_types` are `"bool"`, `"i64"`, `"f64"`, or `"string"`.
     * @param {string} name
     * @param {number} dimension
     * @param {string} metric
     * @param {string[]} col_names
     * @param {string[]} col_types
     */
    create_table_schema(name, dimension, metric, col_names, col_types) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(metric, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayJsValueToWasm0(col_names, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passArrayJsValueToWasm0(col_types, wasm.__wbindgen_malloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_create_table_schema(this.__wbg_ptr, ptr0, len0, dimension, ptr1, len1, ptr2, len2, ptr3, len3);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Records Tombstones for the given ids in `table_name` (FDB-016
     * delete-as-Tombstone semantics for the reduced core). Returns the new
     * in-memory vector count.
     * @param {string} table_name
     * @param {BigUint64Array} ids
     * @returns {number}
     */
    delete_records(table_name, ids) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray64ToWasm0(ids, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_delete_records(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Independent brute-force oracle: scores every vector in `table_name`
     * against `query` using the Table's Metric and returns the `top_k`
     * nearest. Used for exact-match / recall validation distinct from the
     * engine's own search path.
     * @param {string} table_name
     * @param {Float32Array} query
     * @param {number} top_k
     * @returns {SearchHit[]}
     */
    exact_search(table_name, query, top_k) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(query, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_exact_search(this.__wbg_ptr, ptr0, len0, ptr1, len1, top_k);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v3 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v3;
    }
    /**
     * Exact brute-force oracle under the same predicate and knob plumbing as
     * [`FerriteDb::search_advanced`], giving recall@k a like-for-like
     * baseline when filters are active.
     * @param {string} table_name
     * @param {Float32Array} query
     * @param {number} top_k
     * @param {string | null} [predicate_json]
     * @returns {SearchHit[]}
     */
    exact_search_advanced(table_name, query, top_k, predicate_json) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(query, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate_json) ? 0 : passStringToWasm0(predicate_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_exact_search_advanced(this.__wbg_ptr, ptr0, len0, ptr1, len1, top_k, ptr2, len2);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v4 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v4;
    }
    /**
     * Snapshot of the Table's Delta layout for the storage lifecycle
     * inspector (FDB-EXP-07): sealed Segment row/dead counts, active buffer
     * counts, and Tombstone totals.
     * @param {string} table_name
     * @returns {LifecycleExport}
     */
    export_lifecycle(table_name) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_export_lifecycle(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return LifecycleExport.__wrap(ret[0]);
    }
    /**
     * Full snapshot of a Table's in-memory vectors for visualization
     * (FDB-EXP-06): parallel ids/vectors plus per-record metadata JSON.
     * @param {string} table_name
     * @returns {VectorExport}
     */
    export_vectors(table_name) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_export_vectors(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return VectorExport.__wrap(ret[0]);
    }
    /**
     * Appends `vectors.len() / dimension` records to `table_name`. `vectors`
     * is a flat, row-major f32 array aligned with `ids` (one vector per id).
     * Returns the number of vectors now held in the Table.
     * @param {string} table_name
     * @param {BigUint64Array} ids
     * @param {Float32Array} vectors
     * @returns {number}
     */
    insert_records(table_name, ids, vectors) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray64ToWasm0(ids, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF32ToWasm0(vectors, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_insert_records(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Appends records with typed metadata to `table_name`. `vectors` is a
     * flat row-major f32 array aligned with `ids`; `values` is a flat array of
     * `ids.len() * col_names.len()` string literals, one per (record, column),
     * parsed according to `col_types`. This is the ingestion path used by
     * custom datasets and the synthetic generator (FDB-EXP-03).
     * @param {string} table_name
     * @param {BigUint64Array} ids
     * @param {Float32Array} vectors
     * @param {string[]} col_names
     * @param {string[]} col_types
     * @param {string[]} values
     * @returns {number}
     */
    insert_with_metadata(table_name, ids, vectors, col_names, col_types, values) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray64ToWasm0(ids, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayF32ToWasm0(vectors, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passArrayJsValueToWasm0(col_names, wasm.__wbindgen_malloc);
        const len3 = WASM_VECTOR_LEN;
        const ptr4 = passArrayJsValueToWasm0(col_types, wasm.__wbindgen_malloc);
        const len4 = WASM_VECTOR_LEN;
        const ptr5 = passArrayJsValueToWasm0(values, wasm.__wbindgen_malloc);
        const len5 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_insert_with_metadata(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3, ptr4, len4, ptr5, len5);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Names of every Table created in this session, in creation order.
     * @returns {string[]}
     */
    list_tables() {
        const ret = wasm.ferritedb_list_tables(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Creates an empty session.
     */
    constructor() {
        const ret = wasm.ferritedb_new();
        this.__wbg_ptr = ret;
        FerriteDbFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Per-phase execution profile of one exhaustive query (FDB-EXP-08):
     * predicate filtering vs distance scan vs top-k ranking. The phases
     * mirror exactly what the engine's fused [`search`] performs internally,
     * timed here as separate passes over the same Delta for telemetry.
     * @param {string} table_name
     * @param {Float32Array} query
     * @param {number} top_k
     * @param {string | null} [predicate_json]
     * @returns {SearchProfile}
     */
    profile_search(table_name, query, top_k, predicate_json) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(query, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate_json) ? 0 : passStringToWasm0(predicate_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_profile_search(this.__wbg_ptr, ptr0, len0, ptr1, len1, top_k, ptr2, len2);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return SearchProfile.__wrap(ret[0]);
    }
    /**
     * Exhaustive nearest-neighbour search over `table_name`'s in-memory
     * Deltas, returning the `top_k` nearest hits. Mirrors the native engine's
     * admission-gated exhaustive `search`.
     * @param {string} table_name
     * @param {Float32Array} query
     * @param {number} top_k
     * @returns {SearchHit[]}
     */
    search(table_name, query, top_k) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(query, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_search(this.__wbg_ptr, ptr0, len0, ptr1, len1, top_k);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v3 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v3;
    }
    /**
     * Engine search with full query-runner plumbing (FDB-EXP-04): dynamic
     * `SearchOptions` overrides (`probes`, `ef_search`) and a Predicate Tree
     * supplied as JSON (see [`parse_predicate_json`]). On the WASM reduced
     * core the scan is exhaustive, so probe/ef knobs are carried for parity
     * with the native substrate but do not change results.
     * @param {string} table_name
     * @param {Float32Array} query
     * @param {number} top_k
     * @param {number | null} [probes]
     * @param {number | null} [ef_search]
     * @param {string | null} [predicate_json]
     * @returns {SearchHit[]}
     */
    search_advanced(table_name, query, top_k, probes, ef_search, predicate_json) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayF32ToWasm0(query, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate_json) ? 0 : passStringToWasm0(predicate_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_search_advanced(this.__wbg_ptr, ptr0, len0, ptr1, len1, top_k, isLikeNone(probes) ? Number.MAX_SAFE_INTEGER : (probes) >>> 0, isLikeNone(ef_search) ? Number.MAX_SAFE_INTEGER : (ef_search) >>> 0, ptr2, len2);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v4 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v4;
    }
    /**
     * Whole-session status: how many Tables exist and how many vectors are
     * held in total.
     * @returns {SessionStatus}
     */
    status() {
        const ret = wasm.ferritedb_status(this.__wbg_ptr);
        return SessionStatus.__wrap(ret);
    }
    /**
     * Per-Table status: dimension, Metric, and the in-memory vector count.
     * @param {string} table_name
     * @returns {TableStatus}
     */
    table_status(table_name) {
        const ptr0 = passStringToWasm0(table_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.ferritedb_table_status(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return TableStatus.__wrap(ret[0]);
    }
}
if (Symbol.dispose) FerriteDb.prototype[Symbol.dispose] = FerriteDb.prototype.free;

/**
 * Snapshot of one Table's Delta layout for the lifecycle inspector.
 */
export class LifecycleExport {
    static __wrap(ptr) {
        const obj = Object.create(LifecycleExport.prototype);
        obj.__wbg_ptr = ptr;
        LifecycleExportFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        LifecycleExportFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_lifecycleexport_free(ptr, 0);
    }
    /**
     * Tombstoned rows in the active Delta buffer.
     * @returns {number}
     */
    get active_dead() {
        const ret = wasm.lifecycleexport_active_dead(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Rows in the active Delta buffer.
     * @returns {number}
     */
    get active_total() {
        const ret = wasm.lifecycleexport_active_total(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Row count of each sealed Segment, in seal order.
     * @returns {Uint32Array}
     */
    get sealed_counts() {
        const ret = wasm.lifecycleexport_sealed_counts(this.__wbg_ptr);
        var v1 = getArrayU32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Tombstoned row count within each sealed Segment.
     * @returns {Uint32Array}
     */
    get sealed_dead() {
        const ret = wasm.lifecycleexport_sealed_dead(this.__wbg_ptr);
        var v1 = getArrayU32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Number of distinct Tombstoned ids.
     * @returns {number}
     */
    get tombstoned_ids() {
        const ret = wasm.lifecycleexport_tombstoned_ids(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Total recent vectors (sealed + active).
     * @returns {number}
     */
    get total_rows() {
        const ret = wasm.lifecycleexport_total_rows(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) LifecycleExport.prototype[Symbol.dispose] = LifecycleExport.prototype.free;

/**
 * One search result handed back to JS.
 */
export class SearchHit {
    static __wrap(ptr) {
        const obj = Object.create(SearchHit.prototype);
        obj.__wbg_ptr = ptr;
        SearchHitFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SearchHitFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_searchhit_free(ptr, 0);
    }
    /**
     * Distance of the match under the Table's Metric (lower is nearer).
     * @returns {number}
     */
    get distance() {
        const ret = wasm.__wbg_get_searchhit_distance(this.__wbg_ptr);
        return ret;
    }
    /**
     * The matched vector identifier.
     * @returns {bigint}
     */
    get id() {
        const ret = wasm.__wbg_get_searchhit_id(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * The record's metadata as a JSON object string.
     * @returns {string}
     */
    get metadata_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.searchhit_metadata_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Distance of the match under the Table's Metric (lower is nearer).
     * @param {number} arg0
     */
    set distance(arg0) {
        wasm.__wbg_set_searchhit_distance(this.__wbg_ptr, arg0);
    }
    /**
     * The matched vector identifier.
     * @param {bigint} arg0
     */
    set id(arg0) {
        wasm.__wbg_set_searchhit_id(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) SearchHit.prototype[Symbol.dispose] = SearchHit.prototype.free;

/**
 * Per-phase timing breakdown of one exhaustive query (FDB-EXP-08).
 */
export class SearchProfile {
    static __wrap(ptr) {
        const obj = Object.create(SearchProfile.prototype);
        obj.__wbg_ptr = ptr;
        SearchProfileFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SearchProfileFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_searchprofile_free(ptr, 0);
    }
    /**
     * Predicate-filter + Tombstone-visibility pass, microseconds.
     * @returns {bigint}
     */
    get filter_us() {
        const ret = wasm.searchprofile_filter_us(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * @returns {number}
     */
    get matched_rows() {
        const ret = wasm.searchprofile_matched_rows(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Deterministic ranking + truncation pass, microseconds.
     * @returns {bigint}
     */
    get rank_us() {
        const ret = wasm.searchprofile_rank_us(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * @returns {number}
     */
    get returned_rows() {
        const ret = wasm.searchprofile_returned_rows(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Metric distance pass over surviving rows, microseconds.
     * @returns {bigint}
     */
    get scan_us() {
        const ret = wasm.searchprofile_scan_us(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * @returns {number}
     */
    get scanned_rows() {
        const ret = wasm.searchprofile_scanned_rows(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    get total_rows() {
        const ret = wasm.searchprofile_total_rows(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) SearchProfile.prototype[Symbol.dispose] = SearchProfile.prototype.free;

/**
 * Summary of the whole session.
 */
export class SessionStatus {
    static __wrap(ptr) {
        const obj = Object.create(SessionStatus.prototype);
        obj.__wbg_ptr = ptr;
        SessionStatusFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SessionStatusFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_sessionstatus_free(ptr, 0);
    }
    /**
     * Number of Tables created in this session.
     * @returns {number}
     */
    get table_count() {
        const ret = wasm.__wbg_get_sessionstatus_table_count(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Total vectors held across every Table.
     * @returns {number}
     */
    get vector_count() {
        const ret = wasm.__wbg_get_sessionstatus_vector_count(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Number of Tables created in this session.
     * @param {number} arg0
     */
    set table_count(arg0) {
        wasm.__wbg_set_sessionstatus_table_count(this.__wbg_ptr, arg0);
    }
    /**
     * Total vectors held across every Table.
     * @param {number} arg0
     */
    set vector_count(arg0) {
        wasm.__wbg_set_sessionstatus_vector_count(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) SessionStatus.prototype[Symbol.dispose] = SessionStatus.prototype.free;

/**
 * Summary of one Table's in-memory state.
 */
export class TableStatus {
    static __wrap(ptr) {
        const obj = Object.create(TableStatus.prototype);
        obj.__wbg_ptr = ptr;
        TableStatusFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        TableStatusFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_tablestatus_free(ptr, 0);
    }
    /**
     * Fixed vector dimension.
     * @returns {number}
     */
    get dimension() {
        const ret = wasm.tablestatus_dimension(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Fixed distance function (`cosine`, `l2`, or `dot`).
     * @returns {string}
     */
    get metric() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.tablestatus_metric(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Table name.
     * @returns {string}
     */
    get name() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.tablestatus_name(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Number of vectors currently held in memory.
     * @returns {number}
     */
    get vectors() {
        const ret = wasm.tablestatus_vectors(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) TableStatus.prototype[Symbol.dispose] = TableStatus.prototype.free;

/**
 * Snapshot of one Table's vectors for the projection visualizer.
 */
export class VectorExport {
    static __wrap(ptr) {
        const obj = Object.create(VectorExport.prototype);
        obj.__wbg_ptr = ptr;
        VectorExportFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        VectorExportFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_vectorexport_free(ptr, 0);
    }
    /**
     * Record ids, aligned with `vectors` rows and `metadata_json`.
     * @returns {BigUint64Array}
     */
    get ids() {
        const ret = wasm.vectorexport_ids(this.__wbg_ptr);
        return ret;
    }
    /**
     * Per-record metadata serialized as JSON object strings.
     * @returns {string[]}
     */
    get metadata_json() {
        const ret = wasm.vectorexport_metadata_json(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]);
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Flat row-major f32 vector data (`ids.len() * dimension` values).
     * @returns {Float32Array}
     */
    get vectors() {
        const ret = wasm.vectorexport_vectors(this.__wbg_ptr);
        return ret;
    }
}
if (Symbol.dispose) VectorExport.prototype[Symbol.dispose] = VectorExport.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_string_get_d154f1e671052120: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_bb96b2010945f0bc: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_new_from_slice_709ab7061ebcc5da: function(arg0, arg1) {
            const ret = new Float32Array(getArrayF32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_b61d590a0b3abdb3: function(arg0, arg1) {
            const ret = new BigUint64Array(getArrayU64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_searchhit_new: function(arg0) {
            const ret = SearchHit.__wrap(arg0);
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./ferrite_wasm_bg.js": import0,
    };
}

const FerriteDbFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_ferritedb_free(ptr, 1));
const LifecycleExportFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_lifecycleexport_free(ptr, 1));
const SearchHitFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_searchhit_free(ptr, 1));
const SearchProfileFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_searchprofile_free(ptr, 1));
const SessionStatusFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_sessionstatus_free(ptr, 1));
const TableStatusFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_tablestatus_free(ptr, 1));
const VectorExportFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_vectorexport_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigUint64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

let cachedBigUint64ArrayMemory0 = null;
function getBigUint64ArrayMemory0() {
    if (cachedBigUint64ArrayMemory0 === null || cachedBigUint64ArrayMemory0.byteLength === 0) {
        cachedBigUint64ArrayMemory0 = new BigUint64Array(wasm.memory.buffer);
    }
    return cachedBigUint64ArrayMemory0;
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray64ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 8, 8) >>> 0;
    getBigUint64ArrayMemory0().set(arg, ptr / 8);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedBigUint64ArrayMemory0 = null;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('ferrite_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
