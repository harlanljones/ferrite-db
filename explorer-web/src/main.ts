// SPA entry point: dataset management (schema editor, synthetic + custom
// upload ingestion with progress), table switching with session persistence,
// and engine / exact-oracle queries — all driven by the WASM Web Worker.

type WorkerResponse =
  | { id: number; ok: true; payload: unknown }
  | { id: number; ok: true; progress: unknown }
  | { id: number; ok: false; message: string };

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason: string) => void;
  onEvent?: (progress: unknown) => void;
};

const COLUMN_TYPES = ["bool", "i64", "f64", "string"] as const;
const STORE_ACTIVE = "ferrite-explorer-active";
const STORE_CONFIG = "ferrite-explorer-config";
const CHUNK = 2000;

const worker = new Worker(new URL("./search.worker.ts", import.meta.url), {
  type: "module",
});

const pending = new Map<number, Pending>();
let nextId = 1;

worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
  const msg = event.data;
  const entry = pending.get(msg.id);
  if (!entry) return;
  if (msg.ok && "progress" in msg) {
    entry.onEvent?.(msg.progress);
    return;
  }
  pending.delete(msg.id);
  if (msg.ok) entry.resolve((msg as { payload: unknown }).payload);
  else entry.reject((msg as { message: string }).message);
};

worker.onerror = (event) => {
  setStatus("queryStatus", `worker error: ${event.message}`, "bad");
};

function send(type: string, payload: Record<string, unknown> = {}): Promise<unknown> {
  return sendWithEvents(type, payload);
}

function sendWithEvents(
  type: string,
  payload: Record<string, unknown> = {},
  onEvent?: (progress: unknown) => void,
): Promise<unknown> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject, onEvent });
    worker.postMessage({ id, type, ...payload });
  });
}

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing #${id}`);
  return node as T;
}

function setStatus(id: string, text: string, kind: "good" | "bad" | "" = ""): void {
  const node = el<HTMLParagraphElement>(id);
  node.textContent = text;
  node.className = `status${kind ? ` ${kind}` : ""}`;
}

function setResults(value: unknown): void {
  el<HTMLPreElement>("results").textContent = JSON.stringify(value, null, 2);
}

function readNumber(id: string, fallback: number): number {
  const raw = el<HTMLInputElement>(id).value.trim();
  const n = Number(raw);
  return Number.isFinite(n) ? n : fallback;
}

// ---------------------------------------------------------------------------
// Schema editor
// ---------------------------------------------------------------------------

interface ColumnSpec {
  name: string;
  type: string;
}

function addColumnRow(name = "", type = "bool"): void {
  const list = el<HTMLDivElement>("schemaList");
  const row = document.createElement("div");
  row.className = "schema-row";

  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.placeholder = "column name";
  nameInput.value = name;

  const typeSelect = document.createElement("select");
  for (const t of COLUMN_TYPES) {
    const opt = document.createElement("option");
    opt.value = t;
    opt.textContent = t;
    if (t === type) opt.selected = true;
    typeSelect.appendChild(opt);
  }

  const remove = document.createElement("button");
  remove.className = "remove";
  remove.textContent = "×";
  remove.type = "button";
  remove.addEventListener("click", () => row.remove());

  row.append(nameInput, typeSelect, remove);
  list.appendChild(row);
}

function collectSchema(): { columns: ColumnSpec[]; error: string } {
  const rows = Array.from(el<HTMLDivElement>("schemaList").children) as HTMLDivElement[];
  const columns: ColumnSpec[] = [];
  const seen = new Set<string>();
  for (const row of rows) {
    const inputs = row.querySelectorAll("input, select");
    const name = (inputs[0] as HTMLInputElement).value.trim();
    const type = (inputs[1] as HTMLSelectElement).value;
    if (name.length === 0) return { columns, error: "column names must not be empty" };
    if (name.includes("\0")) return { columns, error: `column '${name}' contains a NUL byte` };
    if (!COLUMN_TYPES.includes(type as (typeof COLUMN_TYPES)[number]))
      return { columns, error: `column '${name}' has invalid type '${type}'` };
    if (seen.has(name)) return { columns, error: `duplicate column name '${name}'` };
    seen.add(name);
    columns.push({ name, type });
  }
  return { columns, error: "" };
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

interface SavedConfig {
  metric: string;
  dimension: number;
  count: number;
  seed: number;
  clusters: number;
  schema: ColumnSpec[];
}

function saveConfig(): void {
  const schema = collectSchema().columns;
  const config: SavedConfig = {
    metric: el<HTMLSelectElement>("metric").value,
    dimension: readNumber("dimension", 8),
    count: readNumber("count", 2000),
    seed: readNumber("seed", 42),
    clusters: readNumber("clusters", 5),
    schema,
  };
  try {
    localStorage.setItem(STORE_CONFIG, JSON.stringify(config));
  } catch {
    /* storage may be unavailable; non-fatal */
  }
}

function loadConfig(): void {
  let config: SavedConfig | null = null;
  try {
    const raw = localStorage.getItem(STORE_CONFIG);
    if (raw) config = JSON.parse(raw) as SavedConfig;
  } catch {
    config = null;
  }
  if (!config) return;
  el<HTMLSelectElement>("metric").value = config.metric;
  el<HTMLInputElement>("dimension").value = String(config.dimension);
  el<HTMLInputElement>("count").value = String(config.count);
  el<HTMLInputElement>("seed").value = String(config.seed);
  el<HTMLInputElement>("clusters").value = String(config.clusters);
  for (const col of config.schema) addColumnRow(col.name, col.type);
}

// ---------------------------------------------------------------------------
// Table management
// ---------------------------------------------------------------------------

let activeTable = "";

async function refreshTables(): Promise<void> {
  const res = (await send("listTables")) as { tables: string[] };
  const select = el<HTMLSelectElement>("activeTable");
  const previous = select.value || activeTable;
  select.innerHTML = "";
  for (const name of res.tables) {
    const opt = document.createElement("option");
    opt.value = name;
    opt.textContent = name;
    select.appendChild(opt);
  }
  if (res.tables.includes(previous)) {
    select.value = previous;
    activeTable = previous;
  } else if (res.tables.length > 0) {
    activeTable = res.tables[res.tables.length - 1];
    select.value = activeTable;
  }
}

el<HTMLSelectElement>("activeTable").addEventListener("change", () => {
  activeTable = el<HTMLSelectElement>("activeTable").value;
  try {
    localStorage.setItem(STORE_ACTIVE, activeTable);
  } catch {
    /* non-fatal */
  }
});

el<HTMLButtonElement>("refreshTables").addEventListener("click", () => {
  refreshTables().catch((e) => setStatus("tablesStatus", `refresh failed: ${String(e)}`, "bad"));
});

async function createTable(): Promise<void> {
  const tableName = el<HTMLInputElement>("tableName").value.trim() || "demo";
  const metric = el<HTMLSelectElement>("metric").value;
  const dimension = Math.max(1, Math.floor(readNumber("dimension", 8)));
  const { columns, error } = collectSchema();
  if (error) {
    setStatus("schemaStatus", error, "bad");
    return;
  }
  setStatus("schemaStatus", "creating table…");
  try {
    if (columns.length > 0) {
      const colNames = columns.map((c) => c.name);
      const colTypes = columns.map((c) => c.type);
      await send("createSchema", { name: tableName, dimension, metric, colNames, colTypes });
    } else {
      await send("create", { name: tableName, dimension, metric });
    }
    await refreshTables();
    activeTable = tableName;
    el<HTMLSelectElement>("activeTable").value = tableName;
    try {
      localStorage.setItem(STORE_ACTIVE, tableName);
    } catch {
      /* non-fatal */
    }
    saveConfig();
    setStatus("schemaStatus", `created table '${tableName}'`, "good");
  } catch (e) {
    setStatus("schemaStatus", `create failed: ${String(e)}`, "bad");
  }
}

// ---------------------------------------------------------------------------
// Ingestion (synthetic + upload) with chunked progress
// ---------------------------------------------------------------------------

async function ingestChunks(args: {
  name: string;
  ids: number[];
  vectors: number[];
  colNames: string[];
  colTypes: string[];
  values: string[];
}): Promise<void> {
  const { name, ids, vectors, colNames, colTypes, values } = args;
  const dim = vectors.length / ids.length;
  const total = ids.length;
  const useMeta = colNames.length > 0;
  let done = 0;
  const bar = el<HTMLProgressElement>("ingestBar");
  bar.value = 0;
  try {
    for (let start = 0; start < total; start += CHUNK) {
      const end = Math.min(start + CHUNK, total);
      const sliceIds = ids.slice(start, end);
      const sliceVec = vectors.slice(start * dim, end * dim);
      const sliceVals = useMeta ? values.slice(start * colNames.length, end * colNames.length) : [];
      const payload: Record<string, unknown> = {
        name,
        ids: sliceIds,
        vectors: sliceVec,
        colNames,
        colTypes,
        values: sliceVals,
      };
      if (useMeta) {
        await send("insertMeta", payload);
      } else {
        await send("insert", { name, ids: sliceIds, vectors: sliceVec });
      }
      done = end;
      bar.value = Math.round((done / total) * 100);
      setStatus("ingestStatus", `ingested ${done} / ${total} vectors…`);
    }
    setStatus("ingestStatus", `ingested ${done} vectors into '${name}'`, "good");
    setResults({ table: name, vectors: done });
  } catch (e) {
    bar.value = 0;
    setStatus("ingestStatus", `ingest failed at ${done}/${total}: ${String(e)}`, "bad");
    throw e;
  }
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

function metadataLiteral(type: string, value: unknown): string {
  switch (type) {
    case "bool":
      return value ? "true" : "false";
    case "i64":
    case "f64":
      return String(value);
    case "string":
      return String(value);
    default:
      return String(value);
  }
}

async function ingestSynthetic(): Promise<void> {
  if (!activeTable) {
    setStatus("ingestStatus", "create a table first", "bad");
    return;
  }
  const dimension = Math.max(1, Math.floor(readNumber("dimension", 8)));
  const count = Math.max(1, Math.floor(readNumber("count", 2000)));
  const seed = Math.max(0, Math.floor(readNumber("seed", 42)));
  const clusters = Math.max(1, Math.floor(readNumber("clusters", 5)));
  const { columns } = collectSchema();
  const colNames = columns.map((c) => c.name);
  const colTypes = columns.map((c) => c.type);

  const rng = mulberry32(seed);
  const centroids: number[][] = [];
  for (let c = 0; c < clusters; c++) {
    centroids.push(Array.from({ length: dimension }, () => rng() * 2 - 1));
  }

  const ids: number[] = [];
  const vectors: number[] = [];
  const values: string[] = [];
  for (let i = 0; i < count; i++) {
    const c = i % clusters;
    const center = centroids[c];
    const vec: number[] = [];
    for (let d = 0; d < dimension; d++) vec.push(center[d] + (rng() - 0.5) * 0.5);
    ids.push(i);
    vectors.push(...vec);
    for (const col of columns) {
      if (col.name === "cluster" && col.type === "i64") values.push(String(c));
      else values.push(metadataLiteral(col.type, syntheticValue(col.type, i, rng)));
    }
  }
  setStatus("ingestStatus", `generating ${count} clustered vectors…`);
  await ingestChunks({ name: activeTable, ids, vectors, colNames, colTypes, values });
}

function syntheticValue(type: string, i: number, rng: () => number): unknown {
  switch (type) {
    case "bool":
      return i % 2 === 0;
    case "i64":
      return i;
    case "f64":
      return Number((rng()).toFixed(4));
    case "string":
      return `v${i}`;
    default:
      return i;
  }
}

function parseDatasetFile(fileName: string, text: string, dimension: number): {
  ids: number[];
  vectors: number[];
  colNames: string[];
  colTypes: string[];
  values: string[];
} {
  const isCsv = fileName.toLowerCase().endsWith(".csv");
  if (isCsv) return parseCsv(text, dimension);
  return parseJson(text, dimension);
}

function parseJson(text: string, dimension: number): {
  ids: number[];
  vectors: number[];
  colNames: string[];
  colTypes: string[];
  values: string[];
} {
  const data = JSON.parse(text) as Array<{
    id?: number;
    vector: number[];
    metadata?: Record<string, unknown>;
  }>;
  if (!Array.isArray(data)) throw new Error("JSON root must be an array of records");
  const ids: number[] = [];
  const vectors: number[] = [];
  const values: string[] = [];
  // Infer schema from the first record that has metadata.
  const colNames: string[] = [];
  const colTypes: string[] = [];
  const firstMeta = data.find((d) => d.metadata)?.metadata ?? {};
  for (const [key, val] of Object.entries(firstMeta)) {
    colNames.push(key);
    colTypes.push(inferType(val));
  }
  data.forEach((d, i) => {
    if (!Array.isArray(d.vector) || d.vector.length !== dimension) {
      throw new Error(`record ${i} vector length ${d.vector?.length} != ${dimension}`);
    }
    ids.push(d.id ?? i);
    vectors.push(...d.vector.map(Number));
    const meta = d.metadata ?? {};
    for (const col of colNames) {
      if (!(col in meta)) throw new Error(`record ${i} missing metadata '${col}'`);
      values.push(metadataLiteral(inferType(meta[col]), meta[col]));
    }
  });
  return { ids, vectors, colNames, colTypes, values };
}

function inferType(value: unknown): string {
  if (typeof value === "boolean") return "bool";
  if (typeof value === "number") return Number.isInteger(value) ? "i64" : "f64";
  return "string";
}

function parseCsv(text: string, dimension: number): {
  ids: number[];
  vectors: number[];
  colNames: string[];
  colTypes: string[];
  values: string[];
} {
  const lines = text.trim().split(/\r?\n/);
  if (lines.length < 2) throw new Error("CSV needs a header and at least one row");
  const header = lines[0].split(",").map((s) => s.trim());
  const dimIndex: number[] = [];
  const metaIndex: number[] = [];
  let idIndex = -1;
  header.forEach((h, idx) => {
    if (h === "id") idIndex = idx;
    else if (/^dim\d+$/.test(h)) dimIndex.push(idx);
    else {
      metaIndex.push(idx);
      void h;
    }
  });
  if (dimIndex.length !== dimension) {
    throw new Error(`CSV dim columns (${dimIndex.length}) != table dimension (${dimension})`);
  }
  const colNames = metaIndex.map((idx) => header[idx]);
  const ids: number[] = [];
  const vectors: number[] = [];
  const values: string[] = [];
  lines.slice(1).forEach((line, rowIdx) => {
    const cells = line.split(",");
    if (idIndex >= 0) ids.push(Number(cells[idIndex]));
    else ids.push(rowIdx);
    for (const di of dimIndex) vectors.push(Number(cells[di]));
    for (const mi of metaIndex) {
      const raw = cells[mi].trim();
      values.push(raw);
    }
  });
  // Types inferred from first data row's parseability.
  const colTypes = colNames.map((_, j) => inferCsvType(values[j]));
  return { ids, vectors, colNames, colTypes, values };
}

function inferCsvType(sample: string): string {
  const s = sample.trim().toLowerCase();
  if (s === "true" || s === "false") return "bool";
  if (/^-?\d+$/.test(s)) return "i64";
  if (/^-?\d*\.\d+$/.test(s)) return "f64";
  return "string";
}

async function ingestUpload(): Promise<void> {
  if (!activeTable) {
    setStatus("ingestStatus", "create a table first", "bad");
    return;
  }
  const fileInput = el<HTMLInputElement>("file");
  const file = fileInput.files?.[0];
  if (!file) {
    setStatus("ingestStatus", "choose a JSON or CSV file", "bad");
    return;
  }
  const dimension = Math.max(1, Math.floor(readNumber("dimension", 8)));
  setStatus("ingestStatus", `reading ${file.name}…`);
  try {
    const text = await file.text();
    const parsed = parseDatasetFile(file.name, text, dimension);
    setStatus(
      "ingestStatus",
      `parsed ${parsed.ids.length} records; ingesting…`,
      "good",
    );
    await ingestChunks({
      name: activeTable,
      ids: parsed.ids,
      vectors: parsed.vectors,
      colNames: parsed.colNames,
      colTypes: parsed.colTypes,
      values: parsed.values,
    });
  } catch (e) {
    setStatus("ingestStatus", `upload failed: ${String(e)}`, "bad");
  }
}

el<HTMLButtonElement>("addColumn").addEventListener("click", () => addColumnRow());
el<HTMLButtonElement>("createTable").addEventListener("click", () => {
  createTable().catch((e) => setStatus("schemaStatus", `error: ${String(e)}`, "bad"));
});
el<HTMLButtonElement>("ingest").addEventListener("click", () => {
  const source = (document.querySelector('input[name="source"]:checked') as HTMLInputElement).value;
  saveConfig();
  if (source === "upload") ingestUpload();
  else ingestSynthetic();
});

// Toggle source sub-options.
function syncSource(): void {
  const source = (document.querySelector('input[name="source"]:checked') as HTMLInputElement).value;
  el<HTMLDivElement>("syntheticOpts").hidden = source !== "synthetic";
  el<HTMLDivElement>("uploadOpts").hidden = source !== "upload";
}
document.querySelectorAll('input[name="source"]').forEach((r) =>
  r.addEventListener("change", syncSource),
);

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

function randomVector(dimension: number): number[] {
  const out = new Array<number>(dimension);
  for (let i = 0; i < dimension; i++) out[i] = Math.random() * 2 - 1;
  return out;
}

function parseQuery(dimension: number): number[] | null | undefined {
  const raw = el<HTMLInputElement>("query").value.trim();
  if (!raw) return null;
  const parts = raw.split(",").map((s) => Number(s.trim())).filter((n) => !Number.isNaN(n));
  if (parts.length !== dimension) {
    setStatus(
      "queryStatus",
      `query must have exactly ${dimension} values (got ${parts.length})`,
      "bad",
    );
    return undefined;
  }
  return parts;
}

// ---------------------------------------------------------------------------
// Query: Predicate Tree composer + SearchOptions knobs + recall@k
// ---------------------------------------------------------------------------

const RULE_OPS = ["eq", "ne", "lt", "lte", "gt", "gte", "in"] as const;

function addRuleRow(): void {
  const list = el<HTMLDivElement>("ruleList");
  const row = document.createElement("div");
  row.className = "schema-row rule-row";

  const column = document.createElement("input");
  column.type = "text";
  column.placeholder = "column";

  const op = document.createElement("select");
  for (const candidate of RULE_OPS) {
    const opt = document.createElement("option");
    opt.value = candidate;
    opt.textContent = candidate;
    op.appendChild(opt);
  }

  const value = document.createElement("input");
  value.type = "text";
  value.placeholder = "value (a,b,c for in)";

  row.append(column, op, value);
  // Widen the rule rows to three equal columns like the schema editor.
  const remove = document.createElement("button");
  remove.className = "remove";
  remove.textContent = "×";
  remove.type = "button";
  remove.addEventListener("click", () => row.remove());
  row.appendChild(remove);
  list.appendChild(row);
}

function coerceScalar(raw: string): unknown {
  const t = raw.trim();
  if (t === "true") return true;
  if (t === "false") return false;
  const n = Number(t);
  if (t !== "" && !Number.isNaN(n)) return n;
  return t;
}

function buildPredicateJson(): string | null {
  const rows = Array.from(el<HTMLDivElement>("ruleList").children) as HTMLDivElement[];
  const children: Array<Record<string, unknown>> = [];
  for (const row of rows) {
    const inputs = row.querySelectorAll("input, select");
    const column = (inputs[0] as HTMLInputElement).value.trim();
    const op = (inputs[1] as HTMLSelectElement).value;
    const rawValue = (inputs[2] as HTMLInputElement).value.trim();
    if (column.length === 0) {
      setStatus("ruleStatus", "every rule needs a column name", "bad");
      throw new Error("empty column");
    }
    if (op === "in") {
      children.push({
        op,
        column,
        values: rawValue
          .split(",")
          .filter((part) => part.trim() !== "")
          .map(coerceScalar),
      });
    } else {
      children.push({ op, column, value: coerceScalar(rawValue) });
    }
  }
  setStatus("ruleStatus", "", "");
  if (children.length === 0) return null;
  let tree: Record<string, unknown> =
    children.length === 1 ? children[0] : { op: el<HTMLSelectElement>("rootOp").value, children };
  if (el<HTMLInputElement>("negateRoot").checked) tree = { op: "not", child: tree };
  return JSON.stringify(tree);
}

interface HitView {
  id: string;
  distance: number;
  metadata: unknown;
}

async function runQuery(): Promise<void> {
  if (!activeTable) {
    setStatus("queryStatus", "select or create a table first", "bad");
    return;
  }
  const dimension = Math.max(1, Math.floor(readNumber("dimension", 8)));
  const topK = Math.max(1, Math.floor(readNumber("topK", 10)));
  const probesRaw = el<HTMLInputElement>("probes").value.trim();
  const efRaw = el<HTMLInputElement>("efSearch").value.trim();
  const probes = probesRaw === "" ? null : Math.max(1, Math.floor(Number(probesRaw)));
  const efSearch = efRaw === "" ? null : Math.max(1, Math.floor(Number(efRaw)));
  let query = parseQuery(dimension);
  if (query === undefined) return;
  if (query === null) query = randomVector(dimension);

  let predicateJson: string | null;
  try {
    predicateJson = buildPredicateJson();
  } catch {
    return; // validation message already shown
  }

  setStatus("queryStatus", "searching…");
  try {
    const [engineRes, oracleRes, profile] = await Promise.all([
      send("searchAdv", { name: activeTable, query, topK, probes, efSearch, predicateJson }) as Promise<{
        hits: HitView[];
        latencyMs: number;
      }>,
      send("exactAdv", { name: activeTable, query, topK, predicateJson }) as Promise<{
        hits: HitView[];
        latencyMs: number;
      }>,
      send("profile", { name: activeTable, query, topK, predicateJson }) as Promise<{
        totalRows: number;
        scannedRows: number;
        matchedRows: number;
        returnedRows: number;
        filterUs: number;
        scanUs: number;
        rankUs: number;
      }>,
    ]);
    renderWaterfall(profile);
    const engineIds = new Set(engineRes.hits.map((hit) => hit.id));
    const overlap = oracleRes.hits.filter((hit) => engineIds.has(hit.id)).length;
    const recallAtK = oracleRes.hits.length > 0 ? overlap / oracleRes.hits.length : 1;
    // Feed the projection visualizer: highlight hits + drop the query vector.
    hitDistances.clear();
    for (const hit of engineRes.hits) hitDistances.set(hit.id, hit.distance);
    if (proj && gls) {
      rebuildColors();
      setQueryPoint(applyBasis(query));
      requestRender();
    }
    setStatus(
      "queryStatus",
      `${engineRes.hits.length} hits in ${engineRes.latencyMs.toFixed(2)} ms · ` +
        `oracle ${oracleRes.latencyMs.toFixed(2)} ms · recall@${topK} = ${(recallAtK * 100).toFixed(1)}%`,
      "good",
    );
    setResults({
      recallAtK,
      overlap,
      oracleHits: oracleRes.hits.length,
      knobs: { topK, probes, efSearch },
      predicate: predicateJson ? JSON.parse(predicateJson) : null,
      hits: engineRes.hits,
    });
  } catch (err) {
    setStatus("queryStatus", `query failed: ${String(err)}`, "bad");
  }
}

el<HTMLButtonElement>("addRule").addEventListener("click", () => addRuleRow());
el<HTMLButtonElement>("search").addEventListener("click", () => runQuery());

// ---------------------------------------------------------------------------
// Benchmark suite: streaming run, charts, JSON export
// ---------------------------------------------------------------------------

interface BenchConfig {
  label: string;
  probes: number | null;
  efSearch: number | null;
}

interface BenchReport {
  table: string;
  topK: number;
  passes: number;
  queriesPerPass: number;
  seed: number;
  predicateJson: string | null;
  dimension: number;
  startedAt: string;
  finishedAt: string;
  configs: Array<Record<string, unknown>>;
}

let lastBenchReport: BenchReport | null = null;

function sweepGrid(): BenchConfig[] {
  const grid: BenchConfig[] = [{ label: "auto", probes: null, efSearch: null }];
  for (const probes of [1, 2, 4]) {
    for (const efSearch of [64, 128]) {
      grid.push({ label: `probes=${probes} ef=${efSearch}`, probes, efSearch });
    }
  }
  return grid;
}

interface BenchProgress {
  phase?: "oracle" | "search";
  done?: number;
  total?: number;
  pass?: number;
  passes?: number;
  config?: string;
  latestLatencyMs?: number;
}

function onBenchProgress(progress: unknown): void {
  const p = progress as BenchProgress;
  if (p.phase === "oracle") {
    const done = p.done ?? 0;
    const total = Math.max(1, p.total ?? 1);
    el<HTMLProgressElement>("benchBar").value = (done / total) * 10; // oracle ≈ first 10%
    setStatus("benchStatus", `exact oracle ${done}/${total}…`);
  } else if (p.phase === "search") {
    const done = p.done ?? 0;
    const total = Math.max(1, p.total ?? 1);
    el<HTMLProgressElement>("benchBar").value = 10 + (done / total) * 90;
    setStatus(
      "benchStatus",
      `[${p.config}] pass ${p.pass}/${p.passes} — ${done}/${total} queries · last ${Number(p.latestLatencyMs ?? 0).toFixed(2)} ms`,
    );
  }
}

el<HTMLButtonElement>("runBench").addEventListener("click", () => {
  void runBenchmark();
});

async function runBenchmark(): Promise<void> {
  if (!activeTable) {
    setStatus("benchStatus", "select or create a table first", "bad");
    return;
  }
  let predicateJson: string | null;
  try {
    predicateJson = buildPredicateJson();
  } catch {
    return; // rule validation message already shown
  }
  const payload: Record<string, unknown> = {
    name: activeTable,
    topK: Math.max(1, Math.floor(readNumber("topK", 10))),
    passes: Math.max(1, Math.floor(readNumber("benchPasses", 5))),
    queriesPerPass: Math.max(1, Math.floor(readNumber("benchQueries", 200))),
    seed: Math.max(0, Math.floor(readNumber("benchSeed", 1234))),
    predicateJson,
    configs: el<HTMLInputElement>("benchSweep").checked ? sweepGrid() : undefined,
  };
  el<HTMLButtonElement>("runBench").disabled = true;
  el<HTMLButtonElement>("exportBench").disabled = true;
  el<HTMLProgressElement>("benchBar").value = 0;
  setStatus("benchStatus", "starting benchmark…");
  try {
    const report = (await sendWithEvents("benchmark", payload, onBenchProgress)) as BenchReport;
    lastBenchReport = report;
    drawCharts(report);
    el<HTMLButtonElement>("exportBench").disabled = false;
    el<HTMLProgressElement>("benchBar").value = 100;
    setStatus(
      "benchStatus",
      `done — ${report.passes}×${report.queriesPerPass} queries across ${report.configs.length} configuration(s)`,
      "good",
    );
    setResults(report);
  } catch (err) {
    setStatus("benchStatus", `benchmark failed: ${String(err)}`, "bad");
  } finally {
    el<HTMLButtonElement>("runBench").disabled = false;
  }
}

// --- Canvas charts (dependency-free) ---------------------------------------

function prepCanvas(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  const ratio = window.devicePixelRatio || 1;
  canvas.width = canvas.clientWidth * ratio;
  canvas.height = canvas.clientHeight * ratio;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.scale(ratio, ratio);
  ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
  return ctx;
}

function drawHistogram(canvasId: string, buckets: Array<{ from: number; to: number; count: number }>, unit: string): void {
  const canvas = el<HTMLCanvasElement>(canvasId);
  const ctx = prepCanvas(canvas);
  if (!ctx) return;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.clearRect(0, 0, w, h);
  if (buckets.length === 0) return;
  const pad = { l: 34, r: 8, t: 8, b: 20 };
  const maxCount = Math.max(...buckets.map((b) => b.count), 1);
  const plotW = w - pad.l - pad.r;
  const plotH = h - pad.t - pad.b;
  const bw = plotW / buckets.length;
  ctx.strokeStyle = "#232733";
  ctx.beginPath();
  ctx.moveTo(pad.l, pad.t);
  ctx.lineTo(pad.l, h - pad.b);
  ctx.lineTo(w - pad.r, h - pad.b);
  ctx.stroke();
  ctx.fillStyle = "#6ea8fe";
  buckets.forEach((b, i) => {
    const bh = (b.count / maxCount) * plotH;
    ctx.fillRect(pad.l + i * bw + 1, h - pad.b - bh, Math.max(1, bw - 2), bh);
  });
  ctx.fillStyle = "#9aa3b2";
  ctx.fillText(`${maxCount}`, 4, pad.t + 8);
  ctx.fillText(String(maxCount / 2), 4, pad.t + plotH / 2);
  const fmt = (v: number) => (unit === "%" ? v.toFixed(2) : v.toFixed(v < 10 ? 2 : 0));
  ctx.fillText(fmt(buckets[0].from), pad.l, h - 6);
  const lastLabel = fmt(buckets[buckets.length - 1].to);
  ctx.fillText(lastLabel, w - pad.r - ctx.measureText(lastLabel).width, h - 6);
}

function drawPareto(report: BenchReport): void {
  const canvas = el<HTMLCanvasElement>("pareto");
  const ctx = prepCanvas(canvas);
  if (!ctx) return;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  ctx.clearRect(0, 0, w, h);
  const points = report.configs.map((cfg) => {
    const lat = cfg.latencyMs as { mean: number } | undefined;
    const rec = cfg.recallAtK as { mean: number } | undefined;
    return {
      x: lat?.mean ?? 0,
      y: (rec?.mean ?? 0) * 100,
      label: String(cfg.label ?? ""),
    };
  });
  if (points.length === 0) return;
  const pad = { l: 44, r: 12, t: 12, b: 26 };
  const plotW = w - pad.l - pad.r;
  const plotH = h - pad.t - pad.b;
  const maxX = Math.max(...points.map((p) => p.x)) * 1.15 || 1;
  // Recall axis is fixed to [50,100]% so the Pareto frontier has resolution.
  const yOf = (recallPct: number) =>
    pad.t + ((100 - Math.min(100, Math.max(50, recallPct))) / 50) * plotH;
  const xOf = (lat: number) => pad.l + (lat / maxX) * plotW;
  ctx.strokeStyle = "#232733";
  ctx.beginPath();
  ctx.moveTo(pad.l, pad.t);
  ctx.lineTo(pad.l, h - pad.b);
  ctx.lineTo(w - pad.r, h - pad.b);
  ctx.stroke();
  ctx.strokeStyle = "#2a2f3d";
  for (const g of [60, 70, 80, 90, 100]) {
    const gy = yOf(g);
    ctx.beginPath();
    ctx.moveTo(pad.l, gy);
    ctx.lineTo(w - pad.r, gy);
    ctx.stroke();
    ctx.fillStyle = "#9aa3b2";
    ctx.fillText(`${g}%`, 6, gy + 4);
  }
  points.sort((a, b) => a.x - b.x);
  ctx.strokeStyle = "#7ee787";
  ctx.beginPath();
  points.forEach((p, i) => {
    if (i === 0) ctx.moveTo(xOf(p.x), yOf(p.y));
    else ctx.lineTo(xOf(p.x), yOf(p.y));
  });
  ctx.stroke();
  points.forEach((p, i) => {
    ctx.fillStyle = ["#6ea8fe", "#f0883e", "#bc8cff", "#79c0ff"][i % 4];
    ctx.beginPath();
    ctx.arc(xOf(p.x), yOf(p.y), 4, 0, Math.PI * 2);
    ctx.fill();
  });
  ctx.fillStyle = "#9aa3b2";
  ctx.fillText(`mean latency ms → (max ${maxX.toFixed(2)})`, pad.l, h - 8);
}

function drawCharts(report: BenchReport): void {
  const primary = report.configs[report.configs.length - 1];
  drawHistogram(
    "latencyHist",
    (primary?.latencyHistogram ?? []) as Array<{ from: number; to: number; count: number }>,
    "ms",
  );
  drawHistogram(
    "recallHist",
    (primary?.recallHistogram ?? []) as Array<{ from: number; to: number; count: number }>,
    "%",
  );
  drawPareto(report);
}

el<HTMLButtonElement>("exportBench").addEventListener("click", () => {
  if (!lastBenchReport) return;
  const blob = new Blob([JSON.stringify(lastBenchReport, null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `ferrite-benchmark-${Date.now()}.json`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
});

// ---------------------------------------------------------------------------
// Projection visualizer (FDB-EXP-06): WebGL 2D/3D scatter with PCA basis,
// color-by-metadata, hit highlighting and hover tooltips.
// ---------------------------------------------------------------------------

interface ProjState {
  points: Float32Array; // projected coords, n * comps
  ids: string[];
  metadata: Array<Record<string, unknown>>;
  comps: number;
  explained: number[];
  mean: number[];
  basis: number[][];
}

let proj: ProjState | null = null;
const hitDistances = new Map<string, number>();
let queryPoint: Float32Array | null = null;

interface GlState {
  gl: WebGLRenderingContext;
  program: WebGLProgram;
  posBuf: WebGLBuffer;
  colBuf: WebGLBuffer;
  queryBuf: WebGLBuffer;
  aPos: number;
  aCol: number;
  uMVP: WebGLUniformLocation | null;
  uSize: WebGLUniformLocation | null;
  count: number;
  screen: Float32Array; // per-point clip-derived screen x,y for picking
  cam: { dist: number; yaw: number; pitch: number; panX: number; panY: number };
}

let gls: GlState | null = null;

const PALETTE: Array<[number, number, number]> = [
  [0.43, 0.66, 1.0], [0.99, 0.72, 0.36], [0.64, 0.85, 0.4], [0.93, 0.47, 0.66],
  [0.55, 0.81, 0.85], [0.83, 0.62, 1.0], [0.98, 0.91, 0.42], [0.44, 0.78, 0.55],
  [0.9, 0.59, 0.48], [0.62, 0.71, 0.86],
];

function hashColor(key: string): [number, number, number] {
  let h = 2166136261;
  for (let i = 0; i < key.length; i++) {
    h ^= key.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return PALETTE[(h >>> 0) % PALETTE.length];
}

// --- Minimal column-major mat4 helpers --------------------------------------

function matMul(a: Float32Array, b: Float32Array): Float32Array {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
      out[c * 4 + r] = s;
    }
  }
  return out;
}

function matPerspective(fovy: number, aspect: number, near: number, far: number): Float32Array {
  const f = 1 / Math.tan(fovy / 2);
  const nf = 1 / (near - far);
  return new Float32Array([
    f / aspect, 0, 0, 0,
    0, f, 0, 0,
    0, 0, (far + near) * nf, -1,
    0, 0, 2 * far * near * nf, 0,
  ]);
}

function matOrtho(w: number, h: number): Float32Array {
  return new Float32Array([
    1 / w, 0, 0, 0,
    0, 1 / h, 0, 0,
    0, 0, -0.001, 0,
    0, 0, 0, 1,
  ]);
}

function matRotY(angle: number): Float32Array {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  return new Float32Array([c, 0, -s, 0, 0, 1, 0, 0, s, 0, c, 0, 0, 0, 0, 1]);
}

function matRotX(angle: number): Float32Array {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  return new Float32Array([1, 0, 0, 0, 0, c, s, 0, 0, -s, c, 0, 0, 0, 0, 1]);
}

function matTranslate(x: number, y: number, z: number): Float32Array {
  return new Float32Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1]);
}

// --- GL lifecycle ------------------------------------------------------------

function compile(gl: WebGLRenderingContext, type: number, src: string): WebGLShader {
  const shader = gl.createShader(type)!;
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  return shader;
}

function initGl(): boolean {
  if (gls) return true;
  const canvas = el<HTMLCanvasElement>("projCanvas");
  const gl = canvas.getContext("webgl") as WebGLRenderingContext | null;
  if (!gl) return false;
  const vs = compile(gl, gl.VERTEX_SHADER, `
    attribute vec3 aPos; attribute vec3 aCol;
    uniform mat4 uMVP; uniform float uSize; varying vec3 vCol;
    void main(){ gl_Position = uMVP * vec4(aPos, 1.0); gl_PointSize = uSize; vCol = aCol; }`);
  const fs = compile(gl, gl.FRAGMENT_SHADER, `
    precision mediump float; varying vec3 vCol;
    void main(){ gl_FragColor = vec4(vCol, 1.0); }`);
  const program = gl.createProgram()!;
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  gl.useProgram(program);
  gls = {
    gl,
    program,
    posBuf: gl.createBuffer()!,
    colBuf: gl.createBuffer()!,
    queryBuf: gl.createBuffer()!,
    aPos: gl.getAttribLocation(program, "aPos"),
    aCol: gl.getAttribLocation(program, "aCol"),
    uMVP: gl.getUniformLocation(program, "uMVP"),
    uSize: gl.getUniformLocation(program, "uSize"),
    count: 0,
    screen: new Float32Array(0),
    cam: { dist: 40, yaw: 0.6, pitch: 0.35, panX: 0, panY: 0 },
  };
  requestRender();
  return true;
}

function rebuildColors(): void {
  if (!proj || !gls) return;
  const n = proj.ids.length;
  const colors = new Float32Array(n * 3);
  const by = el<HTMLSelectElement>("projColorBy").value;
  const base: [number, number, number] = [0.53, 0.6, 0.67];
  for (let i = 0; i < n; i++) {
    let c = base;
    if (by !== "__none__") {
      const value = proj.metadata[i]?.[by];
      c = value === undefined ? base : hashColor(String(value));
    }
    if (hitDistances.has(proj.ids[i])) c = [0.94, 0.53, 0.24];
    colors[i * 3] = c[0];
    colors[i * 3 + 1] = c[1];
    colors[i * 3 + 2] = c[2];
  }
  const { gl, colBuf } = gls;
  gl.bindBuffer(gl.ARRAY_BUFFER, colBuf);
  gl.bufferData(gl.ARRAY_BUFFER, colors, gl.DYNAMIC_DRAW);
}

function fitCamera(): void {
  if (!proj || !gls) return;
  let maxR = 1e-6;
  const n = proj.ids.length;
  for (let i = 0; i < n; i++) {
    const x = proj.points[i * proj.comps];
    const y = proj.points[i * proj.comps + 1];
    const z = proj.comps === 3 ? proj.points[i * proj.comps + 2] : 0;
    maxR = Math.max(maxR, Math.abs(x), Math.abs(y), Math.abs(z));
  }
  gls.cam.dist = maxR * 2.6;
  gls.cam.panX = 0;
  gls.cam.panY = 0;
}

function applyBasis(raw: number[]): Float32Array {
  const out = new Float32Array(proj!.comps);
  for (let c = 0; c < proj!.comps; c++) {
    let dot = 0;
    for (let d = 0; d < proj!.mean.length; d++) dot += (raw[d] - proj!.mean[d]) * proj!.basis[c][d];
    out[c] = dot;
  }
  return out;
}

function setQueryPoint(projected: Float32Array): void {
  if (!gls) return;
  const { gl, queryBuf } = gls;
  const xyz = new Float32Array([projected[0], projected[1], projected[2] ?? 0]);
  gl.bindBuffer(gl.ARRAY_BUFFER, queryBuf);
  gl.bufferData(gl.ARRAY_BUFFER, xyz, gl.DYNAMIC_DRAW);
  requestRender();
}

// --- Render loop -------------------------------------------------------------

let renderQueued = false;

function requestRender(): void {
  if (renderQueued || !proj || !gls) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    render();
  });
}

function render(): void {
  const state = gls!;
  const { gl } = state;
  const canvas = el<HTMLCanvasElement>("projCanvas");
  const ratio = window.devicePixelRatio || 1;
  const cw = Math.max(1, Math.floor(canvas.clientWidth * ratio));
  const chh = Math.max(1, Math.floor(canvas.clientHeight * ratio));
  if (canvas.width !== cw || canvas.height !== chh) {
    canvas.width = cw;
    canvas.height = chh;
  }
  gl.viewport(0, 0, cw, chh);
  gl.clearColor(0.047, 0.055, 0.078, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);

  const aspect = cw / chh;
  let mvp: Float32Array;
  if (el<HTMLSelectElement>("projMode").value === "2") {
    const half = state.cam.dist * 0.45;
    mvp = matMul(
      matTranslate(state.cam.panX, state.cam.panY, 0),
      matOrtho(half * aspect, half),
    );
  } else {
    const projM = matPerspective(0.9, aspect, 0.01, 10000);
    const view = matMul(matRotX(state.cam.pitch), matRotY(state.cam.yaw));
    mvp = matMul(projM, matMul(matTranslate(state.cam.panX, state.cam.panY, -state.cam.dist), view));
  }

  gl.useProgram(state.program);
  gl.uniformMatrix4fv(state.uMVP, false, mvp);
  gl.uniform1f(state.uSize, Number(el<HTMLInputElement>("projSize").value));

  gl.bindBuffer(gl.ARRAY_BUFFER, state.posBuf);
  gl.enableVertexAttribArray(state.aPos);
  gl.vertexAttribPointer(state.aPos, 3, gl.FLOAT, false, 0, 0);
  gl.bindBuffer(gl.ARRAY_BUFFER, state.colBuf);
  gl.enableVertexAttribArray(state.aCol);
  gl.vertexAttribPointer(state.aCol, 3, gl.FLOAT, false, 0, 0);
  gl.drawArrays(gl.POINTS, 0, state.count);

  if (queryPoint) {
    gl.uniform1f(state.uSize, Number(el<HTMLInputElement>("projSize").value) + 4);
    gl.bindBuffer(gl.ARRAY_BUFFER, state.queryBuf);
    gl.enableVertexAttribArray(state.aPos);
    gl.vertexAttribPointer(state.aPos, 3, gl.FLOAT, false, 0, 0);
    gl.disableVertexAttribArray(state.aCol);
    gl.vertexAttrib3f(state.aCol, 1.0, 0.25, 0.25);
    gl.drawArrays(gl.POINTS, 0, 1);
    gl.enableVertexAttribArray(state.aCol);
  }

  // CPU-side projection for hover picking.
  const n = state.count;
  if (state.screen.length !== n * 2) state.screen = new Float32Array(n * 2);
  const pts = proj!.points;
  const comps = proj!.comps;
  for (let i = 0; i < n; i++) {
    const x = pts[i * comps];
    const y = pts[i * comps + 1];
    const z = comps === 3 ? pts[i * comps + 2] : 0;
    const cx = mvp[0] * x + mvp[4] * y + mvp[8] * z + mvp[12];
    const cy = mvp[1] * x + mvp[5] * y + mvp[9] * z + mvp[13];
    const cw2 = mvp[3] * x + mvp[7] * y + mvp[11] * z + mvp[15];
    state.screen[i * 2] = ((cx / cw2) * 0.5 + 0.5) * canvas.clientWidth;
    state.screen[i * 2 + 1] = (0.5 - (cy / cw2) * 0.5) * canvas.clientHeight;
  }
}

// --- Interactions ------------------------------------------------------------

function bindProjectionEvents(): void {
  const canvas = el<HTMLCanvasElement>("projCanvas");
  const tip = el<HTMLDivElement>("projTip");
  let dragging: "none" | "orbit" | "pan" = "none";
  let lastX = 0;
  let lastY = 0;

  canvas.addEventListener("mousedown", (e) => {
    dragging = e.shiftKey || e.button === 2 ? "pan" : "orbit";
    lastX = e.clientX;
    lastY = e.clientY;
    if (e.button === 2) e.preventDefault();
  });
  window.addEventListener("mouseup", () => {
    dragging = "none";
  });
  window.addEventListener("mousemove", (e) => {
    if (dragging !== "none" && gls) {
      const dx = e.clientX - lastX;
      const dy = e.clientY - lastY;
      lastX = e.clientX;
      lastY = e.clientY;
      if (dragging === "orbit" && el<HTMLSelectElement>("projMode").value === "3") {
        gls.cam.yaw += dx * 0.008;
        gls.cam.pitch = Math.max(-1.55, Math.min(1.55, gls.cam.pitch + dy * 0.008));
      } else {
        const scale = (gls.cam.dist * 0.0022);
        gls.cam.panX += dx * scale;
        gls.cam.panY -= dy * scale;
      }
      requestRender();
      return;
    }
    // Hover picking against the last rendered screen positions.
    if (!proj || !gls) return;
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    if (mx < 0 || my < 0 || mx > rect.width || my > rect.height) {
      tip.hidden = true;
      return;
    }
    let bestIndex = -1;
    let bestD2 = 12 * 12;
    for (let i = 0; i < proj.ids.length; i++) {
      const dxs = gls.screen[i * 2] - mx;
      const dys = gls.screen[i * 2 + 1] - my;
      const d2 = dxs * dxs + dys * dys;
      if (d2 < bestD2) {
        bestD2 = d2;
        bestIndex = i;
      }
    }
    if (bestIndex < 0) {
      tip.hidden = true;
      return;
    }
    const id = proj.ids[bestIndex];
    const metaEntries = Object.entries(proj.metadata[bestIndex] ?? {}).slice(0, 6);
    const lines = [`<b>id ${id}</b>`];
    const dist = hitDistances.get(id);
    if (dist !== undefined) lines.push(`distance ${dist.toFixed(4)} · hit`);
    for (const [k, v] of metaEntries) lines.push(`${k}: ${String(v)}`);
    tip.innerHTML = lines.join("<br>");
    tip.style.left = `${Math.min(mx + 14, rect.width - 180)}px`;
    tip.style.top = `${Math.min(my + 10, rect.height - 60)}px`;
    tip.hidden = false;
  });
  canvas.addEventListener("mouseleave", () => {
    tip.hidden = true;
  });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas.addEventListener("wheel", (e) => {
    if (!gls) return;
    e.preventDefault();
    gls.cam.dist *= Math.exp(e.deltaY * 0.0012);
    requestRender();
  }, { passive: false });
  canvas.addEventListener("dblclick", () => {
    if (!gls || !proj) return;
    fitCamera();
    gls.cam.yaw = 0.6;
    gls.cam.pitch = 0.35;
    requestRender();
  });
  el<HTMLInputElement>("projSize").addEventListener("input", () => requestRender());
  el<HTMLSelectElement>("projColorBy").addEventListener("change", () => {
    rebuildColors();
    requestRender();
  });
  el<HTMLSelectElement>("projMode").addEventListener("change", () => requestRender());
}

async function runProjectionFlow(): Promise<void> {
  if (!activeTable) {
    setStatus("projStatus", "select or create a table first", "bad");
    return;
  }
  const components = Number(el<HTMLSelectElement>("projMode").value) === 2 ? 2 : 3;
  el<HTMLButtonElement>("runProject").disabled = true;
  setStatus("projStatus", "exporting vectors…");
  try {
    const res = (await sendWithEvents("project", { name: activeTable, components }, (progress) => {
      const p = progress as { phase?: string; component?: number; iteration?: number };
      if (p.phase === "pca") {
        setStatus("projStatus", `PCA component ${p.component}/${components}, iteration ${p.iteration}…`);
      }
    })) as {
      ids: string[];
      points: Float32Array;
      metadata: Array<Record<string, unknown>>;
      components: number;
      explained: number[];
      latencyMs: number;
    };
    hitDistances.clear();
    queryPoint = null;
    proj = {
      points: res.points,
      ids: res.ids,
      metadata: res.metadata,
      comps: res.components,
      explained: res.explained,
      mean: [],
      basis: [],
    };
    if (!initGl()) {
      setStatus("projStatus", "WebGL unavailable in this browser", "bad");
      return;
    }
    // Upload positions padded to vec3 (z = 0 in 2D mode).
    const n = proj.ids.length;
    const pos = new Float32Array(n * 3);
    for (let i = 0; i < n; i++) {
      pos[i * 3] = proj.points[i * proj.comps];
      pos[i * 3 + 1] = proj.points[i * proj.comps + 1];
      pos[i * 3 + 2] = proj.comps === 3 ? proj.points[i * proj.comps + 2] : 0;
    }
    const { gl, posBuf } = gls!;
    gl.bindBuffer(gl.ARRAY_BUFFER, posBuf);
    gl.bufferData(gl.ARRAY_BUFFER, pos, gl.STATIC_DRAW);
    gls!.count = n;

    // Color-by options: union of metadata keys present on the records.
    const select = el<HTMLSelectElement>("projColorBy");
    const previous = select.value;
    const keys = new Set<string>();
    for (const meta of proj.metadata) for (const k of Object.keys(meta ?? {})) keys.add(k);
    select.innerHTML = '<option value="__none__">— none —</option>';
    for (const key of Array.from(keys).sort().slice(0, 50)) {
      const opt = document.createElement("option");
      opt.value = key;
      opt.textContent = key;
      select.appendChild(opt);
    }
    if (Array.from(select.options).some((o) => o.value === previous)) select.value = previous;

    fitCamera();
    rebuildColors();
    requestRender();
    setStatus(
      "projStatus",
      `projected ${n} vectors (${res.components}D) in ${res.latencyMs.toFixed(0)} ms`,
      "good",
    );
  } catch (err) {
    setStatus("projStatus", `projection failed: ${String(err)}`, "bad");
  } finally {
    el<HTMLButtonElement>("runProject").disabled = false;
  }
}

el<HTMLButtonElement>("runProject").addEventListener("click", () => {
  void runProjectionFlow();
});
bindProjectionEvents();

// ---------------------------------------------------------------------------
// Storage lifecycle inspector (FDB-EXP-07): live Delta/Segment snapshot plus
// a client-side simulator of the ratified M4 compaction policy.
// ---------------------------------------------------------------------------

interface SegView {
  label: string;
  rows: number;
  dead: number;
  kind: "sealed" | "merged";
}

interface LifeState {
  segs: SegView[];
  activeRows: number;
  activeDead: number;
  tombstonedIds: number;
  totalRows: number;
}

let life: LifeState | null = null;
let lifeLog: string[] = [];
const PURGE_RATIO = 0.2;

function logLife(message: string): void {
  const time = new Date().toLocaleTimeString();
  lifeLog.unshift(`[${time}] ${message}`);
  lifeLog = lifeLog.slice(0, 8);
}

function deadTotal(state: LifeState): number {
  return state.segs.reduce((acc, s) => acc + s.dead, 0) + state.activeDead;
}

function sealedRows(state: LifeState): number {
  return state.segs.reduce((acc, s) => acc + s.rows, 0);
}

function renderLifecycle(note: string): void {
  if (!life) return;
  const dead = deadTotal(life);
  const ratio = life.totalRows > 0 ? dead / life.totalRows : 0;
  el<HTMLParagraphElement>("lifecycleCounters").textContent =
    `total ${life.totalRows} rows · sealed ${sealedRows(life)} in ${life.segs.length} segment(s) · ` +
    `active ${life.activeRows} · tombstones ${life.tombstonedIds} ids / ${dead} rows · ratio ${(ratio * 100).toFixed(1)}% · ${note}`;
  el<HTMLParagraphElement>("purgeBadge").textContent =
    ratio > PURGE_RATIO
      ? `⚠ tombstone ratio ${(ratio * 100).toFixed(1)}% exceeds ${PURGE_RATIO * 100}% — physical purge ARMED at next merge`
      : ratio > 0
        ? `tombstone ratio ${(ratio * 100).toFixed(1)}% — below ${PURGE_RATIO * 100}% purge threshold (hidden retention at merge)`
        : "";
  el<HTMLParagraphElement>("purgeBadge").className = `status${ratio > PURGE_RATIO ? " bad" : ""}`;

  const chips = el<HTMLDivElement>("segmentChips");
  chips.innerHTML = "";
  const chipsToDraw: Array<{ chip: SegView; extraClass: string }> = life.segs.map((seg) => ({
    chip: seg,
    extraClass: seg.dead > 0 ? "dirty" : "",
  }));
  if (life.activeRows > 0 || life.activeDead > 0) {
    chipsToDraw.push({
      chip: { label: "Δ active", rows: life.activeRows, dead: life.activeDead, kind: "merged" },
      extraClass: "active" + (life.activeDead > 0 ? " dirty" : ""),
    });
  }
  if (chipsToDraw.length === 0) {
    chips.innerHTML = '<span class="hint">empty Delta</span>';
  }
  for (const { chip, extraClass } of chipsToDraw) {
    const div = document.createElement("div");
    div.className = `chip ${chip.kind}${extraClass ? ` ${extraClass}` : ""}`;
    div.style.flexGrow = String(Math.max(1, chip.rows));
    div.textContent = `${chip.label}\n${chip.rows - chip.dead}✓ ${chip.dead}☠`;
    div.title = `${chip.label}: ${chip.rows} rows, ${chip.dead} tombstoned`;
    chips.appendChild(div);
  }

  // Ratified M4 triggers/gates (see crates/ferrite-db/src/compaction.rs).
  const accumulated = sealedRows(life) + life.tombstonedIds;
  const trigger1 = life.totalRows > 0 && accumulated >= Math.max(0.01 * life.totalRows, 100_000);
  const trigger2 = life.segs.length >= 4;
  const gate3 = ratio > PURGE_RATIO;
  const items: Array<[boolean | "gate", string]> = [
    [trigger1, `accumulated change ${accumulated.toLocaleString()} ≥ max(1% of ${life.totalRows}, 100k)`],
    [trigger2, `unmerged Deltas ${life.segs.length} ≥ 4`],
    ["gate", `purge gate: tombstone ratio ${(ratio * 100).toFixed(1)}% ${gate3 ? "> 20% → purge armed" : "≤ 20% → hidden retention"}`],
  ];
  const list = el<HTMLUListElement>("triggerList");
  list.innerHTML = "";
  for (const [fired, text] of items) {
    const li = document.createElement("li");
    li.textContent = `${fired === true ? "✓ fired" : fired === false ? "not armed" : "gate"} — ${text}`;
    if (fired === true) li.className = "fired";
    else if (fired === "gate") li.className = gate3 ? "gated" : "";
    list.appendChild(li);
  }
  el<HTMLPreElement>("lifecycleLog").textContent = lifeLog.join("\n") || "—";
}

async function refreshLifecycle(): Promise<void> {
  if (!activeTable) {
    setStatus("lifecycleStatus", "select or create a table first", "bad");
    return;
  }
  try {
    const snap = (await send("lifecycle", { name: activeTable })) as {
      sealedCounts: number[];
      sealedDead: number[];
      activeTotal: number;
      activeDead: number;
      tombstonedIds: number;
      totalRows: number;
    };
    life = {
      segs: snap.sealedCounts.map((rows, i) => ({
        label: `S${i + 1}`,
        rows,
        dead: snap.sealedDead[i] ?? 0,
        kind: "sealed" as const,
      })),
      activeRows: snap.activeTotal,
      activeDead: snap.activeDead,
      tombstonedIds: snap.tombstonedIds,
      totalRows: snap.totalRows,
    };
    renderLifecycle("live snapshot");
    setStatus("lifecycleStatus", "", "");
    maybeAutoCompact();
  } catch (err) {
    setStatus("lifecycleStatus", `snapshot failed: ${String(err)}`, "bad");
  }
}

/** Applies the ratified M4 merge to the simulated layout (client-side only). */
function simulateCompact(auto: boolean): void {
  if (!life) return;
  const sealed = life.segs.filter((s) => s.kind !== "merged");
  if (sealed.length === 0) {
    logLife(`${auto ? "auto c" : "C"}ompact skipped: no sealed Deltas to absorb`);
    renderLifecycle("compact skipped");
    return;
  }
  const rows = sealedRows(life);
  const dead = life.segs.reduce((acc, s) => acc + s.dead, 0);
  const ratio = life.totalRows > 0 ? dead / life.totalRows : 0;
  const purge = ratio > PURGE_RATIO;
  const merged: SegView = purge
    ? { label: "M1", rows: Math.max(0, rows - dead), dead: 0, kind: "merged" }
    : { label: "M1", rows, dead, kind: "merged" };
  life.segs = [merged];
  // Distinct-id counts aren't derivable from row counts; after a physical
  // purge the surviving Tombstoned ids are those touching the active buffer.
  if (purge) life.tombstonedIds = life.activeDead;
  logLife(
    `${auto ? "auto c" : "C"}ompact: absorbed ${sealed.length} Segment(s) (${rows} rows) into M1 · ` +
      (purge
        ? `ratio ${(ratio * 100).toFixed(1)}% > 20% → purged ${dead} tombstoned rows`
        : `ratio ${(ratio * 100).toFixed(1)}% ≤ 20% → retained ${dead} hidden rows`),
  );
  renderLifecycle("after compact()");
}

function maybeAutoCompact(): void {
  if (!life || !el<HTMLInputElement>("autoCompact").checked) return;
  const accumulated = sealedRows(life) + life.tombstonedIds;
  const trigger1 = life.totalRows > 0 && accumulated >= Math.max(0.01 * life.totalRows, 100_000);
  const trigger2 = life.segs.length >= 4;
  if ((trigger1 || trigger2) && life.segs.length > 0) simulateCompact(true);
}

el<HTMLButtonElement>("refreshLifecycle").addEventListener("click", () => {
  void refreshLifecycle();
});
el<HTMLButtonElement>("deleteBtn").addEventListener("click", async () => {
  if (!activeTable) {
    setStatus("lifecycleStatus", "select or create a table first", "bad");
    return;
  }
  const raw = el<HTMLInputElement>("deleteIds").value.trim();
  if (!raw) {
    setStatus("lifecycleStatus", "enter at least one id", "bad");
    return;
  }
  const ids = raw.split(",").map((s) => Number(s.trim())).filter((n) => Number.isFinite(n));
  try {
    const res = (await send("delete", { name: activeTable, ids })) as {
      deleted: number;
      totalRows: number;
    };
    logLife(`deleted ${res.deleted} id(s) → Tombstones; table now ${res.totalRows} rows`);
    await refreshLifecycle();
    setStatus("lifecycleStatus", `tombstoned ${res.deleted} id(s)`, "good");
  } catch (err) {
    setStatus("lifecycleStatus", `delete failed: ${String(err)}`, "bad");
  }
});
el<HTMLButtonElement>("compactBtn").addEventListener("click", () => {
  if (!life) {
    setStatus("lifecycleStatus", "take a snapshot first", "bad");
    return;
  }
  simulateCompact(false);
});

// ---------------------------------------------------------------------------
// Telemetry polish (FDB-EXP-08): phase waterfall, heap monitoring,
// session cleanup on reload, keyboard shortcuts.
// ---------------------------------------------------------------------------

interface PhaseProfile {
  totalRows: number;
  scannedRows: number;
  matchedRows: number;
  returnedRows: number;
  filterUs: number;
  scanUs: number;
  rankUs: number;
}

function renderWaterfall(p: PhaseProfile): void {
  const box = el<HTMLDivElement>("phaseWaterfall");
  box.hidden = false;
  const phases: Array<[string, number, string]> = [
    [`filter → ${p.matchedRows}/${p.totalRows} rows`, p.filterUs, `predicate + tombstone visibility`],
    [`distance scan → ${p.scannedRows}`, p.scanUs, `metric over surviving rows`],
    [`top-k rank → ${p.returnedRows}`, p.rankUs, `deterministic sort + truncate`],
  ];
  const maxUs = Math.max(p.filterUs, p.scanUs, p.rankUs, 1);
  box.innerHTML = "";
  for (const [label, us, detail] of phases) {
    const row = document.createElement("div");
    row.className = "wf-row";
    const barWrap = document.createElement("div");
    const bar = document.createElement("div");
    bar.className = "wf-bar";
    bar.style.width = `${Math.max(1, (us / maxUs) * 100)}%`;
    barWrap.appendChild(bar);
    const text = document.createElement("span");
    text.textContent = `${(us / 1000).toFixed(2)} ms — ${detail}`;
    row.append(document.createTextNode(label), barWrap, text);
    box.appendChild(row);
  }
}

async function pollHeap(): Promise<void> {
  try {
    const res = (await send("heap")) as { wasmBytes: number | null; jsUsedBytes: number | null };
    const mb = (v: number | null) => (v === null ? "n/a" : `${(v / 1048576).toFixed(1)} MB`);
    el<HTMLSpanElement>("heapChip").textContent =
      `wasm ${mb(res.wasmBytes)} · js ${mb(res.jsUsedBytes)}`;
  } catch {
    /* worker not ready yet */
  }
}

el<HTMLButtonElement>("freeReload").addEventListener("click", async () => {
  try {
    const res = (await send("resetSession")) as {
      wasmBytesBefore: number | null;
      wasmBytesAfter: number | null;
    };
    logLife(
      `session freed & reloaded — wasm memory ${(res.wasmBytesBefore ?? 0) / 1048576}MB → ` +
        `${(res.wasmBytesAfter ?? 0) / 1048576}MB`,
    );
    proj = null;
    hitDistances.clear();
    if (gls) gls.count = 0;
    await refreshTables();
    renderLifecycle("session reloaded");
    setStatus("lifecycleStatus", "session freed and reloaded", "good");
    pollHeap();
  } catch (err) {
    setStatus("lifecycleStatus", `reset failed: ${String(err)}`, "bad");
  }
});

function bindShortcuts(): void {
  window.addEventListener("keydown", (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const target = e.target as HTMLElement | null;
    if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
    switch (e.key) {
      case "/":
        e.preventDefault();
        el<HTMLInputElement>("query").focus();
        break;
      case "s":
        void runQuery();
        break;
      case "b":
        el<HTMLButtonElement>("runBench").click();
        break;
      case "p":
        el<HTMLButtonElement>("runProject").click();
        break;
      case "l":
        void refreshLifecycle();
        break;
      default:
        break;
    }
  });
}
bindShortcuts();

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

async function init(): Promise<void> {
  loadConfig();
  if (el<HTMLDivElement>("schemaList").children.length === 0) addColumnRow();
  if (el<HTMLDivElement>("ruleList").children.length === 0) addRuleRow();
  syncSource();
  try {
    const saved = localStorage.getItem(STORE_ACTIVE);
    if (saved) activeTable = saved;
  } catch {
    /* non-fatal */
  }
  await refreshTables();
  if (activeTable) el<HTMLSelectElement>("activeTable").value = activeTable;
  pollHeap();
  window.setInterval(pollHeap, 2000);
}

init().catch((e) => setStatus("tablesStatus", `init failed: ${String(e)}`, "bad"));
