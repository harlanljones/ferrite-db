// Browser Web Worker: hosts the Ferrite DB WASM engine off the main thread so
// vector ingestion and search never stall the UI. Communicates with the page
// through a tiny request/response protocol over postMessage.

import init, { FerriteDb } from "./pkg/ferrite_wasm.js";
import type { SearchHit } from "./pkg/ferrite_wasm.js";

let db: FerriteDb | null = null;

async function engine(): Promise<FerriteDb> {
  if (!db) {
    await init();
    db = new FerriteDb();
  }
  return db;
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
}

function post(message: unknown): void {
  (self as unknown as Worker).postMessage(message);
}

function serializeHits(hits: SearchHit[]): Array<{ id: string; distance: number }> {
  return hits.map((hit) => ({ id: hit.id.toString(), distance: hit.distance }));
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
