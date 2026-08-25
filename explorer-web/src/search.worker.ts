// Browser Web Worker: hosts the Ferrite DB WASM engine off the main thread so
// vector ingestion and search never stall the UI. Communicates with the page
// through a tiny request/response protocol over postMessage.

import init, { FerriteDb } from "./pkg/ferrite_wasm.js";

let db: FerriteDb | null = null;
let wasmMemory: WebAssembly.Memory | null = null;

async function engine(): Promise<FerriteDb> {
  if (!db) {
    const output = await init();
    wasmMemory = output.memory;
    db = new FerriteDb();
  }
  return db;
}

/** WASM linear memory bytes (never shrinks, but growth is observable). */
function wasmBytes(): number | null {
  return wasmMemory ? wasmMemory.buffer.byteLength : null;
}

/** JS heap usage when the host exposes it (Chromium only). */
function jsHeapBytes(): number | null {
  const perf = performance as Performance & { memory?: { usedJSHeapSize: number } };
  return perf.memory ? perf.memory.usedJSHeapSize : null;
}

/** Frees the current session's WASM object; next `engine()` builds fresh. */
function resetEngine(): void {
  if (db) db.free();
  db = null;
}

interface RequestMessage {
  id: number;
  type:
    | "create"
    | "createSchema"
    | "insert"
    | "insertMeta"
    | "listTables"
    | "search"
    | "exact"
    | "searchAdv"
    | "exactAdv"
    | "benchmark"
    | "project"
    | "profile"
    | "heap"
    | "resetSession"
    | "lifecycle"
    | "delete"
    | "status"
    | "tableStatus";
  name?: string;
  dimension?: number;
  metric?: string;
  colNames?: string[];
  colTypes?: string[];
  ids?: Array<number | bigint>;
  vectors?: number[];
  values?: string[];
  query?: number[];
  topK?: number;
  probes?: number | null;
  efSearch?: number | null;
  predicateJson?: string | null;
  passes?: number;
  queriesPerPass?: number;
  seed?: number;
  configs?: Array<{ label: string; probes: number | null; efSearch: number | null }>;
  components?: number;
}

function post(message: unknown): void {
  (self as unknown as Worker).postMessage(message);
}

function serializeHits(
  hits: Array<{ id: bigint; distance: number; metadata_json: string }>,
): Array<{ id: string; distance: number; metadata: unknown }> {
  return hits.map((hit) => ({
    id: hit.id.toString(),
    distance: hit.distance,
    metadata: JSON.parse(hit.metadata_json || "{}"),
  }));
}

// ---------------------------------------------------------------------------
// Benchmark suite (FDB-EXP-05)
// ---------------------------------------------------------------------------

/// Streams an unsolicited progress frame for a running request; the final
/// `reply` resolves the caller's promise.
function emit(id: number, progress: unknown): void {
  post({ id, ok: true, progress });
}

function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function percentileOf(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1));
  return sorted[index];
}

interface Sample {
  latencyMs: number;
  recall: number;
}

/** Summarizes raw samples into the metrics the report and charts consume. */
function statsOf(samples: Sample[]): Record<string, unknown> {
  const lats = samples.map((s) => s.latencyMs).sort((a, b) => a - b);
  const recalls = samples.map((s) => s.recall).sort((a, b) => a - b);
  const sumLat = lats.reduce((acc, v) => acc + v, 0);
  const meanLat = lats.length > 0 ? sumLat / lats.length : 0;
  const bucketize = (values: number[], bins: number): Array<{ from: number; to: number; count: number }> => {
    if (values.length === 0) return [];
    const min = values[0];
    const max = values[values.length - 1];
    const width = (max - min) / bins || 1;
    const counts = new Array<number>(bins).fill(0);
    for (const v of values) counts[Math.min(bins - 1, Math.floor((v - min) / width))]++;
    return counts.map((count, i) => ({ from: min + i * width, to: min + (i + 1) * width, count }));
  };
  return {
    samples: samples.length,
    latencyMs: {
      min: percentileOf(lats, 0),
      p50: percentileOf(lats, 0.5),
      p90: percentileOf(lats, 0.9),
      p99: percentileOf(lats, 0.99),
      max: percentileOf(lats, 1),
      mean: meanLat,
    },
    qps: sumLat > 0 ? samples.length / (sumLat / 1000) : 0,
    recallAtK: {
      min: percentileOf(recalls, 0),
      p50: percentileOf(recalls, 0.5),
      mean: recalls.length > 0 ? recalls.reduce((a, b) => a + b, 0) / recalls.length : 1,
      max: percentileOf(recalls, 1),
    },
    latencyHistogram: bucketize(lats, 24),
    recallHistogram: bucketize(recalls, 10),
  };
}

async function runBenchmark(msg: RequestMessage, id: number): Promise<unknown> {
  const db = await engine();
  const name = msg.name!;
  const topK = clamp(Math.floor(msg.topK ?? 10), 1, 1000);
  const passes = clamp(Math.floor(msg.passes ?? 5), 1, 100);
  const perPass = clamp(Math.floor(msg.queriesPerPass ?? 200), 1, 5000);
  const seed = Math.floor(msg.seed ?? 42);
  const predicateJson = msg.predicateJson ?? null;
  const configs =
    msg.configs && msg.configs.length > 0
      ? msg.configs
      : [{ label: "auto", probes: null, efSearch: null }];
  const dimension = db.table_status(name).dimension;

  // Deterministic held-out query set: drawn from the benchmark's own seed
  // space, independent of dataset generation.
  const rng = mulberry32(seed);
  const queries: Float32Array[] = [];
  for (let i = 0; i < perPass; i++) {
    const q = new Float32Array(dimension);
    for (let d = 0; d < dimension; d++) q[d] = rng() * 2 - 1;
    queries.push(q);
  }

  // Oracle answers are deterministic per query+predicate: compute once.
  emit(id, { phase: "oracle", done: 0, total: perPass });
  const oracleIds: Array<Set<string>> = [];
  for (let i = 0; i < perPass; i++) {
    const hits = db.exact_search_advanced(name, queries[i], topK, predicateJson);
    oracleIds.push(new Set(hits.map((h) => h.id.toString())));
    if ((i + 1) % 25 === 0 || i === perPass - 1) {
      emit(id, { phase: "oracle", done: i + 1, total: perPass });
    }
  }

  const startedAt = new Date().toISOString();
  const configReports: Array<Record<string, unknown>> = [];
  for (const cfg of configs) {
    const samples: Sample[] = [];
    const total = passes * perPass;
    let done = 0;
    for (let pass = 0; pass < passes; pass++) {
      for (let i = 0; i < perPass; i++) {
        const t0 = performance.now();
        const hits = db.search_advanced(
          name,
          queries[i],
          topK,
          cfg.probes,
          cfg.efSearch,
          predicateJson,
        );
        const latencyMs = performance.now() - t0;
        const ids = new Set(hits.map((h) => h.id.toString()));
        let overlap = 0;
        oracleIds[i].forEach((id2) => {
          if (ids.has(id2)) overlap++;
        });
        const recall = oracleIds[i].size > 0 ? overlap / oracleIds[i].size : 1;
        samples.push({ latencyMs, recall });
        done++;
        if (done % 25 === 0 || done === total) {
          emit(id, {
            phase: "search",
            config: cfg.label,
            pass: pass + 1,
            passes,
            done,
            total,
            latestLatencyMs: latencyMs,
            latestRecall: recall,
          });
        }
      }
    }
    configReports.push({
      label: cfg.label,
      probes: cfg.probes,
      efSearch: cfg.efSearch,
      ...statsOf(samples),
    });
  }

  return {
    kind: "ferrite-benchmark-report",
    version: 1,
    table: name,
    topK,
    passes,
    queriesPerPass: perPass,
    seed,
    predicateJson,
    dimension,
    startedAt,
    finishedAt: new Date().toISOString(),
    configs: configReports,
  };
}

// ---------------------------------------------------------------------------
// Projection (FDB-EXP-06): deterministic PCA via power iteration + deflation
// ---------------------------------------------------------------------------

/** Rows sampled (deterministic stride) for covariance estimation. */
const PCA_MAX_COV_ROWS = 20_000;
const PCA_MAX_ITERATIONS = 60;
const PCA_CONVERGENCE_DELTA = 1e-6;

async function runProjection(msg: RequestMessage, id: number): Promise<unknown> {
  const db = await engine();
  const components = msg.components === 2 ? 2 : 3;
  const t0 = performance.now();
  const exp = db.export_vectors(msg.name!);
  const ids = Array.from(exp.ids, (v) => v.toString());
  const vectors = exp.vectors;
  const metadata = exp.metadata_json.map((s) => JSON.parse(s || "{}") as Record<string, unknown>);
  const n = ids.length;
  if (n === 0) throw new Error(`table '${msg.name}' has no vectors`);
  const dimension = vectors.length / n;

  // Mean-center using a deterministic stride sample for the covariance when
  // the table is large; every point is still projected through the basis.
  const stride = Math.max(1, Math.ceil(n / PCA_MAX_COV_ROWS));
  const mean = new Float64Array(dimension);
  let sampled = 0;
  for (let r = 0; r < n; r += stride) {
    for (let d = 0; d < dimension; d++) mean[d] += vectors[r * dimension + d];
    sampled++;
  }
  for (let d = 0; d < dimension; d++) mean[d] /= sampled;

  const cov = new Float64Array(dimension * dimension);
  for (let r = 0; r < n; r += stride) {
    const off = r * dimension;
    const centeredRow = new Float64Array(dimension);
    for (let d = 0; d < dimension; d++) centeredRow[d] = vectors[off + d] - mean[d];
    for (let i = 0; i < dimension; i++) {
      const vi = centeredRow[i];
      if (vi === 0) continue;
      for (let j = i; j < dimension; j++) cov[i * dimension + j] += vi * centeredRow[j];
    }
  }
  for (let i = 0; i < dimension; i++) {
    for (let j = i; j < dimension; j++) {
      cov[i * dimension + j] /= sampled;
      cov[j * dimension + i] = cov[i * dimension + j];
    }
  }

  // Power iteration with deflation; deterministic golden-ratio start vector.
  const work = Float64Array.from(cov);
  const basis: Float64Array[] = [];
  const explained: number[] = [];
  for (let c = 0; c < components; c++) {
    const v = new Float64Array(dimension);
    for (let d = 0; d < dimension; d++) {
      v[d] = (((d + 1) * 0.6180339887 + c * 0.38196601125) % 1) - 0.5 || 1e-3;
    }
    let lambda = 0;
    for (let iter = 0; iter < PCA_MAX_ITERATIONS; iter++) {
      const w = new Float64Array(dimension);
      for (let i = 0; i < dimension; i++) {
        let sum = 0;
        for (let j = 0; j < dimension; j++) sum += work[i * dimension + j] * v[j];
        w[i] = sum;
      }
      let norm = 0;
      for (let d = 0; d < dimension; d++) norm += w[d] * w[d];
      norm = Math.sqrt(norm);
      if (!Number.isFinite(norm) || norm === 0) break;
      let delta = 0;
      for (let d = 0; d < dimension; d++) {
        w[d] /= norm;
        delta += Math.abs(w[d] - v[d]);
      }
      v.set(w);
      lambda = norm;
      emit(id, { phase: "pca", component: c + 1, components, iteration: iter + 1 });
      if (delta < PCA_CONVERGENCE_DELTA) break;
    }
    for (let i = 0; i < dimension; i++) {
      for (let j = 0; j < dimension; j++) work[i * dimension + j] -= lambda * v[i] * v[j];
    }
    basis.push(v);
    explained.push(lambda);
  }

  const points = new Float32Array(n * components);
  for (let r = 0; r < n; r++) {
    for (let c = 0; c < components; c++) {
      let dot = 0;
      const off = r * dimension;
      for (let d = 0; d < dimension; d++) dot += (vectors[off + d] - mean[d]) * basis[c][d];
      points[r * components + c] = dot;
    }
  }

  return {
    ids,
    points,
    metadata,
    components,
    explained,
    mean: Array.from(mean),
    basis: basis.map((b) => Array.from(b)),
    latencyMs: performance.now() - t0,
  };
}

self.onmessage = async (event: MessageEvent<RequestMessage>) => {
  const msg = event.data;
  const reply = (payload: unknown) => post({ id: msg.id, ok: true, payload });
  const fail = (message: string) => post({ id: msg.id, ok: false, message });

  try {
    const db = await engine();
    switch (msg.type) {
      case "create": {
        const t0 = performance.now();
        db.create_table(msg.name!, msg.dimension!, msg.metric!);
        reply({ latencyMs: performance.now() - t0 });
        break;
      }
      case "insert": {
        const t0 = performance.now();
        const ids = BigUint64Array.from((msg.ids ?? []).map((x) => BigInt(x)));
        const vectors = Float32Array.from(msg.vectors ?? []);
        const count = db.insert_records(msg.name!, ids, vectors);
        reply({ count, latencyMs: performance.now() - t0 });
        break;
      }
      case "createSchema": {
        const t0 = performance.now();
        db.create_table_schema(
          msg.name!,
          msg.dimension!,
          msg.metric!,
          msg.colNames ?? [],
          msg.colTypes ?? [],
        );
        reply({ latencyMs: performance.now() - t0 });
        break;
      }
      case "insertMeta": {
        const t0 = performance.now();
        const ids = BigUint64Array.from((msg.ids ?? []).map((x) => BigInt(x)));
        const vectors = Float32Array.from(msg.vectors ?? []);
        const count = db.insert_with_metadata(
          msg.name!,
          ids,
          vectors,
          msg.colNames ?? [],
          msg.colTypes ?? [],
          msg.values ?? [],
        );
        reply({ count, latencyMs: performance.now() - t0 });
        break;
      }
      case "listTables": {
        reply({ tables: db.list_tables() });
        break;
      }
      case "searchAdv": {
        const t0 = performance.now();
        const hits = db.search_advanced(
          msg.name!,
          Float32Array.from(msg.query!),
          msg.topK!,
          msg.probes ?? null,
          msg.efSearch ?? null,
          msg.predicateJson ?? null,
        );
        const latencyMs = performance.now() - t0;
        reply({ hits: serializeHits(hits), latencyMs });
        break;
      }
      case "exactAdv": {
        const t0 = performance.now();
        const hits = db.exact_search_advanced(
          msg.name!,
          Float32Array.from(msg.query!),
          msg.topK!,
          msg.predicateJson ?? null,
        );
        const latencyMs = performance.now() - t0;
        reply({ hits: serializeHits(hits), latencyMs });
        break;
      }
      case "benchmark": {
        const report = await runBenchmark(msg, msg.id);
        reply(report);
        break;
      }
      case "project": {
        reply(await runProjection(msg, msg.id));
        break;
      }
      case "profile": {
        const p = db.profile_search(
          msg.name!,
          Float32Array.from(msg.query!),
          msg.topK!,
          msg.predicateJson ?? null,
        );
        reply({
          totalRows: p.total_rows,
          scannedRows: p.scanned_rows,
          matchedRows: p.matched_rows,
          returnedRows: p.returned_rows,
          filterUs: Number(p.filter_us),
          scanUs: Number(p.scan_us),
          rankUs: Number(p.rank_us),
        });
        break;
      }
      case "heap": {
        reply({ wasmBytes: wasmBytes(), jsUsedBytes: jsHeapBytes() });
        break;
      }
      case "resetSession": {
        const before = wasmBytes();
        resetEngine();
        await engine();
        reply({ freed: true, wasmBytesBefore: before, wasmBytesAfter: wasmBytes() });
        break;
      }
      case "lifecycle": {
        const snap = db.export_lifecycle(msg.name!);
        reply({
          sealedCounts: Array.from(snap.sealed_counts),
          sealedDead: Array.from(snap.sealed_dead),
          activeTotal: snap.active_total,
          activeDead: snap.active_dead,
          tombstonedIds: snap.tombstoned_ids,
          totalRows: snap.total_rows,
        });
        break;
      }
      case "delete": {
        const ids = BigUint64Array.from((msg.ids ?? []).map((x) => BigInt(x)));
        const count = db.delete_records(msg.name!, ids);
        reply({ deleted: ids.length, totalRows: count });
        break;
      }
      case "search": {
        const t0 = performance.now();
        const hits = db.search(msg.name!, Float32Array.from(msg.query!), msg.topK!);
        const latencyMs = performance.now() - t0;
        reply({ hits: serializeHits(hits), latencyMs });
        break;
      }
      case "exact": {
        const t0 = performance.now();
        const hits = db.exact_search(msg.name!, Float32Array.from(msg.query!), msg.topK!);
        const latencyMs = performance.now() - t0;
        reply({ hits: serializeHits(hits), latencyMs });
        break;
      }
      case "status": {
        const s = db.status();
        reply({ tableCount: s.table_count, vectorCount: s.vector_count });
        break;
      }
      case "tableStatus": {
        const t = db.table_status(msg.name!);
        reply({
          name: t.name,
          dimension: t.dimension,
          metric: t.metric,
          vectors: t.vectors,
        });
        break;
      }
      default:
        fail(`unknown request type: ${(msg as RequestMessage).type}`);
    }
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }
};
