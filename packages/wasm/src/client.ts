import BrowserWorker from "./engine.worker.ts?worker";
import type {
  WorkerMethod,
  WorkerOutboundMessage,
  WorkerRequest,
  WorkerResponse,
} from "./messages.js";
import {
  fromWireObject,
  fromWireHookDef,
  fromWireSearchResults,
  toWireAreaQuery,
  toWireGeofenceDef,
  toWireNearbyQuery,
  toWireSearchOptions,
  toWireSetObjectRequest,
  type AreaQuery,
  type BoundingBox,
  type CollectionInfo,
  type CollectionStats,
  type GeofenceDef,
  type GeofenceEvent,
  type HookDef,
  type HookInfo,
  type LatLngObject,
  type LatLngWasmEventMap,
  type NearbyQuery,
  type SearchOptions,
  type SearchResults,
  type ServerInfo,
  type SetObjectOptions,
  type GeoObject,
} from "./types.js";

/** Configuration for `createLatLng`. */
export interface CreateLatLngOptions {
  /**
   * Explicit URL for `latlng_core_bg.wasm`.
   *
   * Use this when your bundler or CDN serves the wasm file from a custom
   * location. If omitted, the bundled worker resolves `../wasm/latlng_core_bg.wasm`
   * relative to its own emitted worker URL.
   */
  wasmUrl?: string | URL;
  /**
   * Custom worker factory for tests or advanced bundlers.
   *
   * Normal browser applications should omit this and let the package create its
   * bundled module worker.
   */
  workerFactory?: () => Worker;
}

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

/**
 * Browser-only latlng client backed by a dedicated Web Worker and WebAssembly engine.
 *
 * Every API method is asynchronous because requests are sent to the worker with
 * `postMessage`. The main thread never owns or calls the wasm engine directly.
 */
export class LatLngWasmClient {
  readonly #worker: Worker;
  readonly #target = new EventTarget();
  readonly #pending = new Map<number, PendingRequest>();
  readonly #ready: Promise<void>;
  #nextId = 1;
  #closed = false;

  /** Creates a client around a Web Worker. Prefer `createLatLng()` in application code. */
  public constructor(worker: Worker, options: Pick<CreateLatLngOptions, "wasmUrl"> = {}) {
    this.#worker = worker;
    this.#worker.addEventListener("message", this.#handleMessage);
    this.#worker.addEventListener("error", this.#handleError);
    this.#ready = this.#request(
      "init",
      options.wasmUrl ? String(options.wasmUrl) : null,
    ).then(() => undefined);
  }

  /** Resolves when the worker has loaded and initialized the wasm engine. */
  public ready(): Promise<void> {
    return this.#ready;
  }

  /** Creates a collection explicitly. Object writes still create missing collections implicitly. */
  public async createCollection(collection: string): Promise<boolean> {
    return Boolean(await this.#request("createCollection", collection));
  }

  /** Drops a collection and all objects stored in it. */
  public async dropCollection(collection: string): Promise<boolean> {
    return Boolean(await this.#request("dropCollection", collection));
  }

  /** Lists collection names matching a glob pattern. Defaults to all collections. */
  public async collections(pattern = "*"): Promise<string[]> {
    return (await this.#request("collections", pattern)) as string[];
  }

  /** Returns collection bounds and statistics, or `null` if the collection does not exist. */
  public async collectionInfo(collection: string): Promise<CollectionInfo | null> {
    return (await this.#request("collectionInfo", collection)) as CollectionInfo | null;
  }

  /** Stores or updates an object. */
  public async setObject(
    collection: string,
    id: string,
    object: GeoObject,
    options?: SetObjectOptions,
  ): Promise<boolean> {
    return Boolean(
      await this.#request(
        "setObject",
        collection,
        id,
        toWireSetObjectRequest(object, options),
      ),
    );
  }

  /** Stores or updates a point object. */
  public async setPoint(
    collection: string,
    id: string,
    coordinates: { lat: number; lon: number; z?: number | null },
    options?: SetObjectOptions,
  ): Promise<boolean> {
    return this.setObject(
      collection,
      id,
      { type: "point", lat: coordinates.lat, lon: coordinates.lon, z: coordinates.z },
      options,
    );
  }

  /** Returns an object by ID, or `null` when it does not exist. */
  public async getObject(
    collection: string,
    id: string,
    options: { withFields?: boolean } = {},
  ): Promise<LatLngObject | null> {
    return fromWireObject(
      await this.#request("getObject", collection, id, options.withFields ?? true),
    );
  }

  /** Deletes an object by ID. */
  public async deleteObject(collection: string, id: string): Promise<boolean> {
    return Boolean(await this.#request("deleteObject", collection, id));
  }

  /** Sets a TTL in seconds for an existing object. */
  public async expire(collection: string, id: string, seconds: number): Promise<boolean> {
    return Boolean(await this.#request("expire", collection, id, seconds));
  }

  /** Removes TTL metadata from an existing object. */
  public async persist(collection: string, id: string): Promise<boolean> {
    return Boolean(await this.#request("persist", collection, id));
  }

  /** Returns the remaining TTL in seconds, or `null` when no TTL is set or the object is missing. */
  public async ttl(collection: string, id: string): Promise<number | null> {
    const value = await this.#request("ttl", collection, id);
    return typeof value === "number" ? value : null;
  }

  /** Creates or replaces a browser geofence hook. Matching events are emitted as JS events. */
  public async setHook(name: string, def: GeofenceDef): Promise<boolean> {
    return Boolean(await this.#request("setHook", name, { def: toWireGeofenceDef(def) }));
  }

  /** Deletes a browser geofence hook. */
  public async deleteHook(name: string): Promise<boolean> {
    return Boolean(await this.#request("deleteHook", name));
  }

  /** Lists browser geofence hooks matching a glob pattern. */
  public async hooks(pattern = "*"): Promise<HookInfo[]> {
    return (await this.#request("hooks", pattern)) as HookInfo[];
  }

  /** Returns a full browser geofence hook definition, or `null` when it does not exist. */
  public async getHook(name: string): Promise<HookDef | null> {
    return fromWireHookDef(await this.#request("getHook", name));
  }

  /** Runs a nearby spatial query. */
  public async nearby(collection: string, query: NearbyQuery): Promise<SearchResults> {
    return fromWireSearchResults(
      await this.#request("nearby", collection, toWireNearbyQuery(query)),
    );
  }

  /** Runs a within spatial query. */
  public async within(collection: string, query: AreaQuery): Promise<SearchResults> {
    return fromWireSearchResults(
      await this.#request("within", collection, toWireAreaQuery(query)),
    );
  }

  /** Runs an intersects spatial query. */
  public async intersects(collection: string, query: AreaQuery): Promise<SearchResults> {
    return fromWireSearchResults(
      await this.#request("intersects", collection, toWireAreaQuery(query)),
    );
  }

  /** Scans a collection. */
  public async scan(collection: string, options?: SearchOptions): Promise<SearchResults> {
    return fromWireSearchResults(
      await this.#request("scan", collection, toWireSearchOptions(options)),
    );
  }

  /** Scans string objects in a collection. */
  public async search(collection: string, options?: SearchOptions): Promise<SearchResults> {
    return fromWireSearchResults(
      await this.#request("search", collection, toWireSearchOptions(options)),
    );
  }

  /** Returns collection bounds, or `null` when the collection does not exist. */
  public async bounds(collection: string): Promise<BoundingBox | null> {
    return (await this.#request("bounds", collection)) as BoundingBox | null;
  }

  /** Returns statistics for existing collections. */
  public async stats(collections: string[]): Promise<CollectionStats[]> {
    return (await this.#request("stats", collections)) as CollectionStats[];
  }

  /** Returns engine metadata for the in-browser wasm engine. */
  public async serverInfo(): Promise<ServerInfo> {
    return (await this.#request("serverInfo")) as ServerInfo;
  }

  /** Registers a typed event listener for mutation or geofence events. */
  public addEventListener<K extends keyof LatLngWasmEventMap>(
    type: K,
    listener: (event: LatLngWasmEventMap[K]) => void,
    options?: AddEventListenerOptions,
  ): void {
    this.#target.addEventListener(type, listener as EventListener, options);
  }

  /** Removes a previously registered typed event listener. */
  public removeEventListener<K extends keyof LatLngWasmEventMap>(
    type: K,
    listener: (event: LatLngWasmEventMap[K]) => void,
    options?: EventListenerOptions,
  ): void {
    this.#target.removeEventListener(type, listener as EventListener, options);
  }

  /** Convenience alias for `addEventListener`. */
  public on<K extends keyof LatLngWasmEventMap>(
    type: K,
    listener: (event: LatLngWasmEventMap[K]) => void,
  ): () => void {
    this.addEventListener(type, listener);
    return () => this.removeEventListener(type, listener);
  }

  /** Terminates the worker and rejects pending requests. The client cannot be reused afterward. */
  public close(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#worker.removeEventListener("message", this.#handleMessage);
    this.#worker.removeEventListener("error", this.#handleError);
    this.#worker.terminate();
    for (const pending of this.#pending.values()) {
      pending.reject(new Error("latlng wasm client is closed"));
    }
    this.#pending.clear();
  }

  #request(method: WorkerMethod, ...params: unknown[]): Promise<unknown> {
    if (this.#closed) {
      return Promise.reject(new Error("latlng wasm client is closed"));
    }
    const id = this.#nextId++;
    const request: WorkerRequest = { id, method, params };
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#worker.postMessage(request);
    });
  }

  #handleMessage = (message: MessageEvent<WorkerOutboundMessage>): void => {
    const data = message.data;
    if ("type" in data && data.type === "event") {
      if (data.eventType === "mutation") {
        this.#target.dispatchEvent(new CustomEvent("mutation", { detail: data.event }));
        return;
      }
      this.#target.dispatchEvent(new CustomEvent("geofence", { detail: data.event }));
      this.#target.dispatchEvent(
        new CustomEvent(`geofence:${data.event.detect}`, { detail: data.event }),
      );
      return;
    }

    this.#resolveResponse(data as WorkerResponse);
  };

  #handleError = (event: ErrorEvent): void => {
    const error = new Error(event.message || "latlng wasm worker failed");
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
  };

  #resolveResponse(response: WorkerResponse): void {
    const pending = this.#pending.get(response.id);
    if (!pending) {
      return;
    }
    this.#pending.delete(response.id);
    if (response.ok) {
      pending.resolve(response.result);
    } else {
      pending.reject(new Error(response.error));
    }
  }
}

/** Creates a worker-backed in-browser latlng engine. */
export async function createLatLng(
  options: CreateLatLngOptions = {},
): Promise<LatLngWasmClient> {
  const worker = options.workerFactory?.() ?? new BrowserWorker();
  const client = new LatLngWasmClient(worker, { wasmUrl: options.wasmUrl });
  await client.ready();
  return client;
}
