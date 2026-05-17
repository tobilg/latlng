import init, { BrowserLatLng } from "../pkg/latlng_core.js";
import type {
  WasmMutationResponse,
  WorkerOutboundMessage,
  WorkerRequest,
} from "./messages.js";
import { fromWireGeofenceEvent, type MutationEvent } from "./types.js";

let enginePromise: Promise<BrowserLatLng> | undefined;
let configuredWasmUrl: string | undefined;

function engine(): Promise<BrowserLatLng> {
  const wasmUrl =
    configuredWasmUrl ??
    new URL(["..", "wasm", "latlng_core_bg.wasm"].join("/"), import.meta.url);
  enginePromise ??= init({ module_or_path: wasmUrl }).then(() => new BrowserLatLng());
  return enginePromise;
}

self.addEventListener("message", (message: MessageEvent<WorkerRequest>) => {
  void handleRequest(message.data);
});

async function handleRequest(request: WorkerRequest): Promise<void> {
  try {
    if (request.method === "init") {
      const result = await initialize(request.params);
      post({ id: request.id, ok: true, result });
      return;
    }
    const db = await engine();
    const result = await dispatch(db, request.method, request.params);
    post({ id: request.id, ok: true, result });
  } catch (error) {
    post({
      id: request.id,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

async function initialize(params: unknown[]): Promise<unknown> {
  const [wasmUrl] = expectParams<[string | null]>(params);
  if (!enginePromise && typeof wasmUrl === "string" && wasmUrl.length > 0) {
    configuredWasmUrl = wasmUrl;
  }
  return (await engine()).server_info();
}

async function dispatch(
  db: BrowserLatLng,
  method: WorkerRequest["method"],
  params: unknown[],
): Promise<unknown> {
  switch (method) {
    case "init":
      return initialize(params);
    case "createCollection": {
      const [collection] = expectParams<[string]>(params);
      const response = db.create_collection(collection) as WasmMutationResponse<boolean>;
      emitMutation({
        type: "collection:create",
        collection,
        changed: response.result,
      });
      emitGeofences(response.events);
      return response.result;
    }
    case "dropCollection": {
      const [collection] = expectParams<[string]>(params);
      const response = db.drop_collection(collection) as WasmMutationResponse<boolean>;
      emitMutation({
        type: "collection:drop",
        collection,
        changed: response.result,
      });
      emitGeofences(response.events);
      return response.result;
    }
    case "collections": {
      const [pattern] = expectParams<[string | null]>(params);
      return db.collections(pattern);
    }
    case "collectionInfo": {
      const [collection] = expectParams<[string]>(params);
      return db.collection_info(collection);
    }
    case "setObject": {
      const [collection, id, payload] = expectParams<[string, string, unknown]>(params);
      const response = db.set_object(
        collection,
        id,
        payload,
      ) as WasmMutationResponse<boolean>;
      emitMutation({
        type: "object:set",
        collection,
        id,
        changed: response.result,
      });
      emitGeofences(response.events);
      return response.result;
    }
    case "getObject": {
      const [collection, id, withFields] = expectParams<[string, string, boolean]>(params);
      return db.get_object(collection, id, withFields);
    }
    case "deleteObject": {
      const [collection, id] = expectParams<[string, string]>(params);
      const response = db.delete_object(
        collection,
        id,
      ) as WasmMutationResponse<boolean>;
      emitMutation({
        type: "object:delete",
        collection,
        id,
        changed: response.result,
      });
      emitGeofences(response.events);
      return response.result;
    }
    case "expire": {
      const [collection, id, seconds] = expectParams<[string, string, number]>(params);
      const response = db.expire(collection, id, seconds) as WasmMutationResponse<boolean>;
      emitMutation({ type: "object:expire", collection, id, changed: response.result });
      return response.result;
    }
    case "persist": {
      const [collection, id] = expectParams<[string, string]>(params);
      const response = db.persist(collection, id) as WasmMutationResponse<boolean>;
      emitMutation({ type: "object:persist", collection, id, changed: response.result });
      return response.result;
    }
    case "ttl": {
      const [collection, id] = expectParams<[string, string]>(params);
      return db.ttl(collection, id);
    }
    case "setHook": {
      const [name, def] = expectParams<[string, unknown]>(params);
      db.set_hook(name, def);
      emitMutation({ type: "hook:set", hook: name, changed: true });
      return true;
    }
    case "deleteHook": {
      const [name] = expectParams<[string]>(params);
      const deleted = db.delete_hook(name);
      emitMutation({ type: "hook:delete", hook: name, changed: deleted });
      return deleted;
    }
    case "hooks": {
      const [pattern] = expectParams<[string | null]>(params);
      return db.hooks(pattern);
    }
    case "getHook": {
      const [name] = expectParams<[string]>(params);
      return db.hook(name);
    }
    case "nearby": {
      const [collection, query] = expectParams<[string, unknown]>(params);
      return db.nearby_query(collection, query);
    }
    case "within": {
      const [collection, query] = expectParams<[string, unknown]>(params);
      return db.within_query(collection, query);
    }
    case "intersects": {
      const [collection, query] = expectParams<[string, unknown]>(params);
      return db.intersects_query(collection, query);
    }
    case "scan": {
      const [collection, options] = expectParams<[string, unknown]>(params);
      return db.scan(collection, options);
    }
    case "search": {
      const [collection, options] = expectParams<[string, unknown]>(params);
      return db.search(collection, options);
    }
    case "bounds": {
      const [collection] = expectParams<[string]>(params);
      return db.bounds(collection);
    }
    case "stats": {
      const [collections] = expectParams<[string[]]>(params);
      return db.stats(collections);
    }
    case "serverInfo":
      return db.server_info();
  }
}

function emitMutation(event: MutationEvent): void {
  post({ type: "event", eventType: "mutation", event });
}

function emitGeofences(events: unknown[]): void {
  for (const event of events) {
    post({
      type: "event",
      eventType: "geofence",
      event: fromWireGeofenceEvent(event),
    });
  }
}

function post(message: WorkerOutboundMessage): void {
  self.postMessage(message);
}

function expectParams<T extends unknown[]>(params: unknown[]): T {
  return params as T;
}
