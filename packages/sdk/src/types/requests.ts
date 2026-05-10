import type {
  FieldEntry,
  FieldMap,
  GeoObject,
  NearbyQuery,
  OutputFormat,
  SearchOptions,
} from "./models.js";
import type { GeofenceDef } from "./events.js";

export type ReadPreference =
  | "leader"
  | "leaderPreferred"
  | "followerPreferred"
  | "roundRobinFollowers";

/** Configuration for creating a `LatLngClient`. */
export interface LatLngClientConfig {
  /** Base URL of the primary `latlng` server, for example `http://127.0.0.1:7421`. */
  leaderUrl: string;
  /** Optional follower URLs that may be used for read routing. */
  readReplicas?: string[];
  /** Read routing strategy to use when follower URLs are configured. */
  readPreference?: ReadPreference;
  /** Bearer token or JWT to send with HTTP and WebSocket requests. */
  token?: string;
  /** Per-request HTTP timeout in milliseconds. */
  timeoutMs?: number;
  /** Additional headers to merge into every HTTP request. */
  headers?: HeadersInit;
  /** Custom `fetch` implementation for Node, testing, or custom runtimes. */
  fetch?: typeof globalThis.fetch;
  /** Custom WebSocket constructor/factory, useful for tests or non-browser runtimes. */
  webSocketFactory?: (url: string) => WebSocket;
  /** Cache TTL for follower health/status probes used in replica routing. */
  replicaStatusTtlMs?: number;
}

/** Options for object upserts through `setObject` and `setPoint`. */
export interface SetObjectOptions {
  /** Field values to write alongside the object. */
  fields?: FieldMap | FieldEntry[];
  /** Optional object TTL in seconds. */
  expireSeconds?: number;
  /** Conditional write mode. */
  condition?: "always" | "nx" | "xx";
}

/** Options for `rename`. */
export interface RenameOptions {
  /** When `true`, the rename only succeeds if the target collection does not yet exist. */
  nx?: boolean;
}

/** Options for `get`. */
export interface GetObjectOptions {
  /** Include the stored field map in the response. */
  withFields?: boolean;
  /** Output format requested from the server. */
  format?: Exclude<OutputFormat, "count" | "ids">;
}

/** Options for `deleteMatching`. */
export interface DeleteMatchingOptions {
  /** Match pattern used to select object IDs for deletion. */
  matchPattern?: string;
}

/** Options for `setFields`. */
export interface FsetOptions {
  /** When `true`, only update existing objects. */
  xx?: boolean;
}

/** Options for `setJson`. */
export interface JsetOptions {
  /** When `true`, treat the `value` as raw JSON instead of a string literal. */
  raw?: boolean;
}

/** Area-based query request shared by `within` and `intersects`. */
export interface SearchAreaRequest {
  area: import("./models.js").Area;
  options?: SearchOptions;
}

/** Request body for `timeout`. */
export interface TimeoutRequest {
  /** Command name to configure, for example `set`. */
  command: string;
  /** Timeout in seconds. */
  seconds: number;
}

/** Request body for `setChannel`. */
export interface SetChannelRequest {
  /** Unique channel name. */
  name: string;
  /** Geofence definition bound to the channel. */
  def: GeofenceDef;
}

/** Request body for `setHook`. */
export interface SetHookRequest {
  /** Unique hook name. */
  name: string;
  /** Destination endpoint that receives webhook deliveries. */
  endpoint: string;
  /** Geofence definition bound to the hook. */
  def: GeofenceDef;
}

/** Options for `connectWebSocket`. */
export interface WebSocketConnectOptions {
  /** Override URL for the WebSocket endpoint. Defaults to the leader URL with `/ws`. */
  url?: string;
}

/** Alias for `nearby` request payloads. */
export type NearbyRequest = NearbyQuery;
