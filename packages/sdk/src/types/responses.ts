import type { HookInfo } from "./events.js";
import type {
  BoundingBox,
  CollectionInfo,
  CollectionStats,
  CompactionResult,
  LatLngObject,
  SearchResults,
  ServerInfo,
} from "./models.js";

/** Response returned by `ping`. */
export interface PingResponse {
  ok: true;
  pong: true;
}

/** Generic `{ ok: true }` response shape. */
export interface OkResponse {
  ok: true;
}

/** Response returned by `collections`. */
export interface CollectionsResponse {
  collections: string[];
}

/** Response returned by `rename`. */
export interface RenameResponse {
  ok: true;
  renamed: boolean;
}

/** Response returned by `createCollection`. */
export interface CreateCollectionResponse {
  ok: true;
  created: boolean;
}

/** Response returned by `bounds`. */
export interface BoundsResponse {
  bounds: BoundingBox;
}

/** Response returned by `stats`. */
export type StatsResponse = CollectionStats[];

/** Response returned by object upsert operations. */
export interface SetObjectResponse {
  ok: true;
  stored: boolean;
}

/** Response returned by `get`. */
export type GetObjectResponse = LatLngObject | null;

/** Generic delete response shape used by multiple endpoints. */
export interface DeleteResponse {
  ok?: true;
  deleted: boolean | number;
}

/** Response returned by field mutation endpoints. */
export interface UpdatedResponse {
  ok: true;
  updated: boolean;
}

/** Response returned by field reads. */
export interface FieldResponse {
  value: import("./models.js").FieldValue | null;
}

/** Response returned by `ttl`. */
export interface TtlResponse {
  ttl: number | null;
}

/** Response returned by JSON path reads. */
export interface JsonValueResponse {
  value: unknown;
}

/** Response returned by search endpoints. */
export type SearchResponse = SearchResults;

/** Response returned by `channels`. */
export type ChannelsResponse = string[];

/** Response returned by `hooks`. */
export interface HooksResponse extends Array<HookInfo> {}

/** Response returned by `configGet`. */
export interface ConfigValueResponse {
  value: string;
}

/** Response returned by `webhookQueue`. */
export interface WebhookQueueStatsResponse {
  pending: number;
  leased: number;
  dead_letter: number;
  oldest_pending_age_ms: number | null;
}

/** Response returned by `readonly`. */
export interface ReadOnlyResponse {
  ok: true;
  read_only: boolean;
}

/** Response returned by `timeout`. */
export interface TimeoutResponse {
  ok: true;
  command: string;
  seconds: number | null;
}

/** Response returned by `dropCollection`. */
export interface DropCollectionResponse {
  dropped: boolean;
}

/** Response returned by `getCollection`. */
export type CollectionInfoResponse = CollectionInfo;

/** Response returned by `server`. */
export type ServerResponse = ServerInfo;

/** Response returned by `info`. */
export interface InfoResponse {
  server: ServerInfo;
  metrics?: Record<string, unknown>;
  section?: string;
}

/** Response returned by `metrics`. */
export type MetricsResponse = string;
/** Response returned by `aofshrink`. */
export type AofshrinkResponse = CompactionResult;
