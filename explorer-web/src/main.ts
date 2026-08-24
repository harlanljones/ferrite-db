// SPA entry point: dataset management (schema editor, synthetic + custom
// upload ingestion with progress), table switching with session persistence,
// and engine / exact-oracle queries — all driven by the WASM Web Worker.

type WorkerResponse =
  | { id: number; ok: true; payload: unknown }
  | { id: number; ok: false; message: string };

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason: string) => void;
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
  pending.delete(msg.id);
  if (msg.ok) entry.resolve(msg.payload);
  else entry.reject(msg.message);
};

worker.onerror = (event) => {
  setStatus("queryStatus", `worker error: ${event.message}`, "bad");
};

function send(type: string, payload: Record<string, unknown> = {}): Promise<unknown> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
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

async function runQuery(exact: boolean): Promise<void> {
  if (!activeTable) {
    setStatus("queryStatus", "select or create a table first", "bad");
    return;
  }
  const dimension = Math.max(1, Math.floor(readNumber("dimension", 8)));
  const topK = Math.max(1, Math.floor(readNumber("topK", 10)));
  let query = parseQuery(dimension);
  if (query === undefined) return;
  if (query === null) query = randomVector(dimension);

  setStatus("queryStatus", exact ? "running exact oracle…" : "searching…");
  try {
    const res = (await send(exact ? "exact" : "search", {
      name: activeTable,
      query,
      topK,
    })) as { hits: Array<{ id: string; distance: number }>; latencyMs: number };
    setStatus(
      "queryStatus",
      `${exact ? "exact" : "engine"} returned ${res.hits.length} hits in ${res.latencyMs.toFixed(2)} ms`,
      "good",
    );
    setResults(res.hits);
  } catch (err) {
    setStatus("queryStatus", `query failed: ${String(err)}`, "bad");
  }
}

el<HTMLButtonElement>("search").addEventListener("click", () => runQuery(false));
el<HTMLButtonElement>("exact").addEventListener("click", () => runQuery(true));

el<HTMLButtonElement>("status").addEventListener("click", async () => {
  if (!activeTable) {
    setStatus("queryStatus", "select or create a table first", "bad");
    return;
  }
  try {
    const session = (await send("status")) as { tableCount: number; vectorCount: number };
    let table = null;
    try {
      table = (await send("tableStatus", { name: activeTable })) as {
        name: string;
        dimension: number;
        metric: string;
        vectors: number;
      };
    } catch {
      table = { error: `table '${activeTable}' not found` };
    }
    setResults({ session, table });
  } catch (err) {
    setStatus("queryStatus", `status failed: ${String(err)}`, "bad");
  }
});

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

async function init(): Promise<void> {
  loadConfig();
  if (el<HTMLDivElement>("schemaList").children.length === 0) addColumnRow();
  syncSource();
  try {
    const saved = localStorage.getItem(STORE_ACTIVE);
    if (saved) activeTable = saved;
  } catch {
    /* non-fatal */
  }
  await refreshTables();
  if (activeTable) el<HTMLSelectElement>("activeTable").value = activeTable;
}

init().catch((e) => setStatus("tablesStatus", `init failed: ${String(e)}`, "bad"));
