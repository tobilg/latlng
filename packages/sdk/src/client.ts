import {
  HttpError,
  LatLngError,
  ServerUnavailableError,
  TimeoutError,
} from "./errors/index.js";
import { routes } from "./http/routes.js";
import { HttpTransport } from "./http/transport.js";
import {
  parseBooleanResult,
  parseBounds,
  parseChannelDefinition,
  parseChannels,
  parseCollectionInfo,
  parseCollections,
  parseCompactionResult,
  parseDeleted,
  parseDeletedCount,
  parseDropped,
  parseFieldValueResponse,
  parseHookDefinition,
  parseHookSummaries,
  parseInfo,
  parseJsonValue,
  parseRenamed,
  parseSeconds,
  parseStats,
  parseStored,
  parseTtl,
  parseUpdated,
  parseWebhookQueueStats,
} from "./internal/response-parsers.js";
import { ReplicaRouter } from "./routing/replicas.js";
import { LatLngWebSocketClient } from "./ws/socket.js";
import type { ChannelDef, HookDef, HookInfo } from "./types/events.js";
import { toWireGeofenceDef } from "./types/events.js";
import {
  fromWireObject,
  fromWireSearchResults,
  parseServerInfo,
  point,
  toWireFieldEntries,
  toWireGeoObject,
  toWireSearchOptions,
  toWireArea,
  type CollectionStats,
  type CollectionInfo,
  type CompactionResult,
  type FieldEntry,
  type FieldMap,
  type FieldValue,
  type GeoObject,
  type LatLngObject,
  type SearchOptions,
  type SearchResults,
  type ServerInfo,
} from "./types/models.js";
import type {
  DeleteMatchingOptions,
  FsetOptions,
  GetObjectOptions,
  JsetOptions,
  LatLngClientConfig,
  RenameOptions,
  SearchAreaRequest,
  SetHookRequest,
  SetObjectOptions,
  SetChannelRequest,
  TimeoutRequest,
  WebSocketConnectOptions,
} from "./types/requests.js";
import type {
  ChannelsResponse,
  InfoResponse,
  WebhookQueueStatsResponse,
} from "./types/responses.js";

function conditionToWire(
  condition: SetObjectOptions["condition"] | undefined,
): string {
  switch (condition) {
    case "nx":
      return "Nx";
    case "xx":
      return "Xx";
    default:
      return "Always";
  }
}

function outputFormatToQuery(
  format: GetObjectOptions["format"],
): { format?: string; hash_precision?: number } {
  if (!format || format === "objects") {
    return { format: "objects" };
  }
  if (format === "points" || format === "bounds") {
    return { format };
  }
  if ("hashes" in format) {
    return { format: "hashes", hash_precision: format.hashes.precision };
  }
  return { format: "objects" };
}

/**
 * High-level HTTP and WebSocket client for a `latlng` server.
 */
export class LatLngClient {
  private readonly transport: HttpTransport;
  private readonly replicaRouter: ReplicaRouter;
  private readonly token?: string;
  private readonly webSocketFactory?: (url: string) => WebSocket;

  /**
   * Creates a new client instance for a `latlng` deployment.
   */
  public constructor(config: LatLngClientConfig) {
    this.transport = new HttpTransport({
      baseUrl: config.leaderUrl,
      token: config.token,
      timeoutMs: config.timeoutMs,
      headers: config.headers,
      fetch: config.fetch,
    });
    this.replicaRouter = new ReplicaRouter({
      leaderUrl: config.leaderUrl,
      readReplicas: config.readReplicas,
      readPreference: config.readPreference,
      statusTtlMs: config.replicaStatusTtlMs,
    });
    this.token = config.token;
    this.webSocketFactory = config.webSocketFactory;
  }

  /**
   * Verifies that the configured leader is reachable and authenticated.
   *
   * @returns `{ ok: true, pong: true }` when the leader responds successfully.
   */
  public async ping(): Promise<{ ok: true; pong: true }> {
    return this.requestLeader({
      path: routes.ping(),
      parser: (value) => value as { ok: true; pong: true },
    });
  }

  /**
   * Returns the server health response for the configured leader.
   *
   * @returns `{ ok: true }` when the leader reports healthy status.
   */
  public async healthz(): Promise<{ ok: true }> {
    return this.requestLeader({
      path: routes.healthz(),
      parser: (value) => value as { ok: true },
    });
  }

  /**
   * Returns the current server status for the configured leader.
   *
   * @returns Parsed server status object.
   */
  public async server(): Promise<ServerInfo> {
    return this.requestLeader({ path: routes.server(), parser: parseServerInfo });
  }

  /**
   * Returns the server info response, optionally scoped to a section.
   *
   * @param section Optional info section requested from the server.
   * @returns Parsed info response.
   */
  public async info(section?: string): Promise<InfoResponse> {
    return this.requestLeader({ path: routes.info(section), parser: parseInfo });
  }

  /**
   * Returns the current Prometheus text exposition metrics from the configured leader.
   *
   * @returns Metrics in Prometheus text exposition format.
   */
  public async metrics(): Promise<string> {
    return this.transport.requestText({ path: routes.metrics() });
  }

  /**
   * Lists visible collection names, optionally filtered by a match pattern.
   *
   * @param matchPattern Optional glob-like match pattern for collection names.
   * @returns Visible collection names.
   */
  public async collections(matchPattern?: string): Promise<string[]> {
    return this.requestRead({
      path: routes.collections(matchPattern),
      parser: parseCollections,
    });
  }

  /**
   * Creates an empty collection explicitly.
   *
   * @param collection Collection name.
   * Returns `true` when the collection was created and `false` when it already existed.
   * @returns Whether a new collection was created.
   */
  public async createCollection(collection: string): Promise<boolean> {
    return this.requestWrite({
      method: "POST",
      path: routes.collection(collection),
      parser: (value) => parseBooleanResult(value, "created"),
    });
  }

  /**
   * Returns collection metadata for a single collection, or `null` when it does not exist.
   *
   * @param collection Collection name.
   * @returns Collection metadata or `null`.
   */
  public async getCollection(collection: string): Promise<CollectionInfo | null> {
    try {
      return await this.requestRead({
        path: routes.collection(collection),
        parser: parseCollectionInfo,
      });
    } catch (error) {
      if (error instanceof HttpError && error.status === 404) {
        return null;
      }
      throw error;
    }
  }

  /**
   * Renames a collection.
   *
   * @param collection Source collection name.
   * @param newName Target collection name.
   * @param options Rename options.
   * @returns Whether the rename was applied.
   */
  public async rename(
    collection: string,
    newName: string,
    options?: RenameOptions,
  ): Promise<boolean> {
    return this.requestWrite({
      method: "POST",
      path: routes.renameCollection(collection),
      body: { new_name: newName, nx: options?.nx ?? false },
      parser: parseRenamed,
    });
  }

  /**
   * Returns the geographic bounds for a collection.
   *
   * @param collection Collection name.
   * @returns Bounding box for the collection.
   */
  public async bounds(collection: string) {
    return this.requestRead({ path: routes.bounds(collection), parser: parseBounds });
  }

  /**
   * Returns collection statistics.
   *
   * @param collection Collection name.
   * @returns Collection statistics array from the server.
   */
  public async stats(collection: string): Promise<CollectionStats[]> {
    return this.requestRead({ path: routes.stats(collection), parser: parseStats });
  }

  /**
   * Stores or updates a single object in a collection.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param object Object payload.
   * @param options Write options.
   * @returns Whether the object was stored.
   */
  public async setObject(
    collection: string,
    id: string,
    object: GeoObject,
    options?: SetObjectOptions,
  ): Promise<boolean> {
    return this.requestWrite({
      method: "POST",
      path: routes.object(collection, id),
      body: {
        object: toWireGeoObject(object),
        fields: toWireFieldEntries(options?.fields),
        expire_seconds: options?.expireSeconds,
        condition: conditionToWire(options?.condition),
      },
      parser: parseStored,
    });
  }

  /**
   * Convenience wrapper for storing a point object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param coordinates Point coordinates.
   * @param options Write options.
   * @returns Whether the point was stored.
   */
  public async setPoint(
    collection: string,
    id: string,
    coordinates: { lat: number; lon: number; z?: number | null },
    options?: SetObjectOptions,
  ): Promise<boolean> {
    return this.setObject(
      collection,
      id,
      point(coordinates.lat, coordinates.lon, coordinates.z),
      options,
    );
  }

  /**
   * Fetches a single object by collection and ID.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param options Read options.
   * Returns `null` when the object does not exist.
   * @returns Parsed object or `null`.
   */
  public async get(
    collection: string,
    id: string,
    options?: GetObjectOptions,
  ): Promise<LatLngObject | null> {
    const output = outputFormatToQuery(options?.format);
    return this.requestRead({
      path: `${routes.object(collection, id)}${routesQuery({
        with_fields: options?.withFields ?? false,
        format: output.format,
        hash_precision: output.hash_precision,
      })}`,
      parser: fromWireObject,
    });
  }

  /**
   * Deletes a single object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @returns Whether an object was deleted.
   */
  public async delete(collection: string, id: string): Promise<boolean> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.object(collection, id),
      parser: parseDeleted,
    });
  }

  /**
   * Deletes objects matching the supplied ID pattern and returns the deleted count.
   *
   * @param collection Collection name.
   * @param options Delete options.
   * @returns Number of deleted objects.
   */
  public async deleteMatching(
    collection: string,
    options?: DeleteMatchingOptions,
  ): Promise<number> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.objects(collection, options?.matchPattern),
      parser: parseDeletedCount,
    });
  }

  /**
   * Updates one or more fields on an existing object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param fields Field values to write.
   * @param options Field update options.
   * @returns Whether the fields were updated.
   */
  public async setFields(
    collection: string,
    id: string,
    fields: FieldMap | FieldEntry[],
    options?: FsetOptions,
  ): Promise<boolean> {
    return this.requestWrite({
      method: "POST",
      path: routes.fields(collection, id),
      body: {
        fields: toWireFieldEntries(fields),
        xx: options?.xx ?? false,
      },
      parser: parseUpdated,
    });
  }

  /**
   * Fetches a single field value from an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param field Field name.
   * @returns Field value or `null`.
   */
  public async getField(
    collection: string,
    id: string,
    field: string,
  ): Promise<FieldValue | null> {
    return this.requestRead({
      path: routes.field(collection, id, field),
      parser: parseFieldValueResponse,
    });
  }

  /**
   * Sets or replaces the TTL for an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param seconds TTL in seconds.
   */
  public async expire(
    collection: string,
    id: string,
    seconds: number,
  ): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.expire(collection, id),
      body: { seconds },
      parser: () => undefined,
    });
  }

  /**
   * Removes any TTL from an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   */
  public async persist(collection: string, id: string): Promise<void> {
    await this.requestWrite({
      method: "DELETE",
      path: routes.expire(collection, id),
      parser: () => undefined,
    });
  }

  /**
   * Returns the remaining TTL in seconds for an object, or `null` if no TTL is set.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @returns Remaining TTL in seconds or `null`.
   */
  public async ttl(collection: string, id: string): Promise<number | null> {
    return this.requestRead({ path: routes.ttl(collection, id), parser: parseTtl });
  }

  /**
   * Writes a JSON value at the given JSON path on an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param path JSON path.
   * @param value JSON value string or raw payload, depending on `options.raw`.
   * @param options JSON write options.
   */
  public async setJson(
    collection: string,
    id: string,
    path: string,
    value: string,
    options?: JsetOptions,
  ): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.jsonRoot(collection, id),
      body: { path, value, raw: options?.raw ?? false },
      parser: () => undefined,
    });
  }

  /**
   * Reads a JSON value from the given JSON path on an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param path JSON path.
   * @returns Parsed JSON value.
   */
  public async getJson(
    collection: string,
    id: string,
    path: string,
  ): Promise<unknown> {
    return this.requestRead({
      path: routes.jsonPath(collection, id, path),
      parser: parseJsonValue,
    });
  }

  /**
   * Deletes a JSON value at the given JSON path on an object.
   *
   * @param collection Collection name.
   * @param id Object identifier.
   * @param path JSON path.
   * @returns Whether a JSON value was deleted.
   */
  public async deleteJson(
    collection: string,
    id: string,
    path: string,
  ): Promise<boolean> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.jsonPath(collection, id, path),
      parser: (value) => parseBooleanResult(value, "deleted"),
    });
  }

  /**
   * Runs a `NEARBY` search against a collection.
   *
   * @param collection Collection name.
   * @param query Nearby query payload.
   * @returns Search results.
   */
  public async nearby(
    collection: string,
    query: import("./types/models.js").NearbyQuery,
  ): Promise<SearchResults> {
    return this.requestRead({
      method: "POST",
      path: routes.nearby(collection),
      body: {
        lat: query.lat,
        lon: query.lon,
        meters: query.meters,
        options: toWireSearchOptions(query.options),
      },
      parser: fromWireSearchResults,
    });
  }

  /**
   * Runs a `WITHIN` search against a collection.
   *
   * @param collection Collection name.
   * @param request Area search request.
   * @returns Search results.
   */
  public async within(
    collection: string,
    request: SearchAreaRequest,
  ): Promise<SearchResults> {
    return this.requestRead({
      method: "POST",
      path: routes.within(collection),
      body: {
        area: toWireArea(request.area),
        options: toWireSearchOptions(request.options),
      },
      parser: fromWireSearchResults,
    });
  }

  /**
   * Runs an `INTERSECTS` search against a collection.
   *
   * @param collection Collection name.
   * @param request Area search request.
   * @returns Search results.
   */
  public async intersects(
    collection: string,
    request: SearchAreaRequest,
  ): Promise<SearchResults> {
    return this.requestRead({
      method: "POST",
      path: routes.intersects(collection),
      body: {
        area: toWireArea(request.area),
        options: toWireSearchOptions(request.options),
      },
      parser: fromWireSearchResults,
    });
  }

  /**
   * Runs a `SCAN` query against a collection.
   *
   * @param collection Collection name.
   * @param options Search options.
   * @returns Search results.
   */
  public async scan(
    collection: string,
    options?: SearchOptions,
  ): Promise<SearchResults> {
    return this.requestRead({
      method: "POST",
      path: routes.scan(collection),
      body: toWireSearchOptions(options),
      parser: fromWireSearchResults,
    });
  }

  /**
   * Runs a text search query against a collection.
   *
   * @param collection Collection name.
   * @param options Search options.
   * @returns Search results.
   */
  public async search(
    collection: string,
    options?: SearchOptions,
  ): Promise<SearchResults> {
    return this.requestRead({
      method: "POST",
      path: routes.search(collection),
      body: toWireSearchOptions(options),
      parser: fromWireSearchResults,
    });
  }

  /**
   * Creates or replaces a geofence channel definition.
   *
   * @param request Channel definition request.
   * Returns the stored channel name.
   * @returns Stored channel name.
   */
  public async setChannel(request: SetChannelRequest): Promise<string> {
    return this.requestWrite({
      method: "POST",
      path: routes.channels(),
      body: {
        name: request.name,
        def: toWireGeofenceDef(request.def),
      },
      parser: (value) => {
        if (!value || typeof value !== "object" || !("name" in value)) {
          throw new LatLngError("invalid set channel response");
        }
        return String((value as { name?: unknown }).name ?? "");
      },
    });
  }

  /**
   * Deletes a channel definition by name.
   *
   * @param name Channel name.
   * @returns Whether a channel was deleted.
   */
  public async deleteChannel(name: string): Promise<boolean> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.channel(name),
      parser: parseDeleted,
    });
  }

  /**
   * Lists visible channel names, optionally filtered by a match pattern.
   *
   * @param matchPattern Optional glob-like match pattern for channel names.
   * @returns Visible channel names.
   */
  public async channels(matchPattern?: string): Promise<ChannelsResponse> {
    return this.requestRead({
      path: routes.channels(matchPattern),
      parser: parseChannels,
    });
  }

  /**
   * Returns a full channel definition by name, or `null` when it does not exist.
   *
   * @param name Channel name.
   * @returns Full channel definition or `null`.
   */
  public async getChannel(name: string): Promise<ChannelDef | null> {
    try {
      return await this.requestRead({
        path: routes.channel(name),
        parser: parseChannelDefinition,
      });
    } catch (error) {
      if (error instanceof HttpError && error.status === 404) {
        return null;
      }
      throw error;
    }
  }

  /**
   * Creates or replaces a webhook hook definition.
   *
   * @param request Hook definition request.
   */
  public async setHook(request: SetHookRequest): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.hooks(),
      body: {
        name: request.name,
        endpoint: request.endpoint,
        def: toWireGeofenceDef(request.def),
      },
      parser: () => undefined,
    });
  }

  /**
   * Deletes a hook definition by name.
   *
   * @param name Hook name.
   * @returns Whether a hook was deleted.
   */
  public async deleteHook(name: string): Promise<boolean> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.hook(name),
      parser: parseDeleted,
    });
  }

  /**
   * Lists visible hook summaries, optionally filtered by a match pattern.
   *
   * @param matchPattern Optional glob-like match pattern for hook names.
   * @returns Visible hook summaries.
   */
  public async hooks(matchPattern?: string): Promise<HookInfo[]> {
    return this.requestRead({
      path: routes.hooks(matchPattern),
      parser: parseHookSummaries,
    });
  }

  /**
   * Returns a full hook definition by name, or `null` when it does not exist.
   *
   * @param name Hook name.
   * @returns Full hook definition or `null`.
   */
  public async getHook(name: string): Promise<HookDef | null> {
    try {
      return await this.requestRead({
        path: routes.hook(name),
        parser: parseHookDefinition,
      });
    } catch (error) {
      if (error instanceof HttpError && error.status === 404) {
        return null;
      }
      throw error;
    }
  }

  /**
   * Reads a single runtime configuration value from the leader.
   *
   * @param name Config key.
   * @returns Config value as a string.
   */
  public async configGet(name: string): Promise<string> {
    return this.requestLeader({
      path: routes.config(name),
      parser: (value) => {
        if (!value || typeof value !== "object" || !("value" in value)) {
          throw new LatLngError("invalid config response");
        }
        return String((value as { value?: unknown }).value ?? "");
      },
    });
  }

  /**
   * Updates a runtime configuration value on the leader.
   *
   * @param name Config key.
   * @param value Config value.
   */
  public async configSet(name: string, value: string): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.config(name),
      body: { value },
      parser: () => undefined,
    });
  }

  /**
   * Persists the current runtime configuration to disk.
   */
  public async configRewrite(): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.configRewrite(),
      parser: () => undefined,
    });
  }

  /**
   * Flushes all data from the database.
   */
  public async flushdb(): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.flushdb(),
      parser: () => undefined,
    });
  }

  /**
   * Runs garbage collection / maintenance on the leader.
   */
  public async gc(): Promise<void> {
    await this.requestWrite({
      method: "POST",
      path: routes.gc(),
      parser: () => undefined,
    });
  }

  /**
   * Triggers AOF compaction and returns before/after statistics.
   *
   * @returns Compaction result statistics.
   */
  public async aofshrink(): Promise<CompactionResult> {
    return this.requestWrite({
      method: "POST",
      path: routes.aofshrink(),
      parser: parseCompactionResult,
    });
  }

  /**
   * Returns webhook queue statistics from the leader.
   *
   * @returns Webhook queue statistics.
   */
  public async webhookQueue(): Promise<WebhookQueueStatsResponse> {
    return this.requestLeader({
      path: routes.webhookQueue(),
      parser: parseWebhookQueueStats,
    });
  }

  /**
   * Enables or disables read-only mode on the leader.
   *
   * @param enabled Whether read-only mode should be enabled.
   * @returns Effective read-only state.
   */
  public async readonly(enabled: boolean): Promise<boolean> {
    return this.requestWrite({
      method: "POST",
      path: routes.readonly(),
      body: { enabled },
      parser: (value) => {
        if (!value || typeof value !== "object" || !("read_only" in value)) {
          throw new LatLngError("invalid readonly response");
        }
        return Boolean((value as { read_only?: unknown }).read_only);
      },
    });
  }

  /**
   * Configures per-command server timeouts.
   *
   * @param request Timeout request payload.
   * @returns Effective timeout in seconds, or `null` when unset.
   */
  public async timeout(request: TimeoutRequest): Promise<number | null> {
    return this.requestWrite({
      method: "POST",
      path: routes.timeout(),
      body: request,
      parser: parseSeconds,
    });
  }

  /**
   * Drops a collection explicitly.
   *
   * @param collection Collection name.
   * @returns Whether a collection was dropped.
   */
  public async dropCollection(collection: string): Promise<boolean> {
    return this.requestWrite({
      method: "DELETE",
      path: routes.collection(collection),
      parser: parseDropped,
    });
  }

  /**
   * Opens a WebSocket client for subscriptions against the configured leader.
   *
   * @param options WebSocket connection options.
   * @returns Connected WebSocket client.
   */
  public async connectWebSocket(
    options?: WebSocketConnectOptions,
  ): Promise<LatLngWebSocketClient> {
    return LatLngWebSocketClient.connect({
      url: options?.url ?? toWebSocketUrl(this.replicaRouter.getLeaderUrl()),
      token: this.token,
      webSocketFactory: this.webSocketFactory,
    });
  }

  private async requestWrite<T>(options: {
    method?: "GET" | "POST" | "DELETE";
    path: string;
    body?: unknown;
    parser: (value: unknown) => T;
  }): Promise<T> {
    return this.transport.request(options);
  }

  private async requestLeader<T>(options: {
    method?: "GET" | "POST" | "DELETE";
    path: string;
    body?: unknown;
    parser: (value: unknown) => T;
  }): Promise<T> {
    return this.transport.request(options);
  }

  private async requestRead<T>(options: {
    method?: "GET" | "POST" | "DELETE";
    path: string;
    body?: unknown;
    parser: (value: unknown) => T;
  }): Promise<T> {
    const preference = this.replicaRouter.getReadPreference();
    if (preference === "leader") {
      return this.transport.request(options);
    }

    const replicas = await this.replicaRouter.getReadCandidates(this.transport);
    if (preference === "leaderPreferred") {
      try {
        return await this.requestLeader(options);
      } catch (error) {
        if (!isRetryableReadRoutingError(error)) {
          throw error;
        }
      }
      for (const replica of replicas) {
        try {
          return await this.transport.request({ ...options, baseUrl: replica });
        } catch (error) {
          if (!isRetryableReadRoutingError(error)) {
            throw error;
          }
        }
      }
      return this.requestLeader(options);
    }

    for (const replica of replicas) {
      try {
        return await this.transport.request({ ...options, baseUrl: replica });
      } catch (error) {
        if (!isRetryableReadRoutingError(error)) {
          throw error;
        }
      }
    }
    return this.requestLeader(options);
  }
}

function isRetryableReadRoutingError(error: unknown): boolean {
  return (
    error instanceof ServerUnavailableError ||
    error instanceof TimeoutError ||
    !(error instanceof HttpError)
  );
}

function routesQuery(
  query: Record<string, string | number | boolean | undefined>,
): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined) {
      continue;
    }
    params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : "";
}

function toWebSocketUrl(httpUrl: string): string {
  const url = new URL(httpUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = routes.ws();
  url.search = "";
  url.hash = "";
  return url.toString();
}
