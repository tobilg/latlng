import { LatLngError } from "../errors/index.js";
import { parseChannelDef, parseHookDef, parseHookInfoList } from "../types/events.js";
import type { ChannelDef, HookDef, HookInfo } from "../types/events.js";
import {
  fromWireFieldValue,
  parseServerInfo,
  type BoundingBox,
  type CollectionInfo,
  type CollectionStats,
  type CompactionResult,
  type FieldValue,
} from "../types/models.js";
import type {
  ChannelsResponse,
  InfoResponse,
  WebhookQueueStatsResponse,
} from "../types/responses.js";

export function parseCollections(value: unknown): string[] {
  if (
    value &&
    typeof value === "object" &&
    "collections" in value &&
    Array.isArray((value as { collections?: unknown[] }).collections)
  ) {
    return (value as { collections: unknown[] }).collections.filter(
      (entry): entry is string => typeof entry === "string",
    );
  }
  throw new LatLngError("invalid collections response");
}

export function parseChannels(value: unknown): ChannelsResponse {
  if (
    value &&
    typeof value === "object" &&
    "channels" in value &&
    Array.isArray((value as { channels?: unknown[] }).channels)
  ) {
    return (value as { channels: unknown[] }).channels.filter(
      (entry): entry is string => typeof entry === "string",
    );
  }
  throw new LatLngError("invalid channels response");
}

export function parseStats(value: unknown): CollectionStats[] {
  if (!Array.isArray(value)) {
    throw new LatLngError("invalid stats response");
  }
  return value as CollectionStats[];
}

export function parseInfo(value: unknown): InfoResponse {
  if (!value || typeof value !== "object" || !("server" in value)) {
    throw new LatLngError("invalid info response");
  }
  const record = value as Record<string, unknown>;
  return {
    server: parseServerInfo(record.server),
    metrics:
      record.metrics && typeof record.metrics === "object"
        ? (record.metrics as Record<string, unknown>)
        : undefined,
    section: typeof record.section === "string" ? record.section : undefined,
  };
}

export function parseBounds(value: unknown): BoundingBox {
  if (
    value &&
    typeof value === "object" &&
    "bounds" in value &&
    value.bounds &&
    typeof value.bounds === "object"
  ) {
    return value.bounds as BoundingBox;
  }
  throw new LatLngError("invalid bounds response");
}

export function parseCollectionInfo(value: unknown): CollectionInfo {
  if (
    !value ||
    typeof value !== "object" ||
    !("name" in value) ||
    typeof (value as { name?: unknown }).name !== "string" ||
    !("stats" in value) ||
    !(value as { stats?: unknown }).stats ||
    typeof (value as { stats?: unknown }).stats !== "object"
  ) {
    throw new LatLngError("invalid collection info response");
  }

  return value as CollectionInfo;
}

export function parseWebhookQueueStats(value: unknown): WebhookQueueStatsResponse {
  if (!value || typeof value !== "object") {
    throw new LatLngError("invalid webhook queue response");
  }
  return value as WebhookQueueStatsResponse;
}

export function parseCompactionResult(value: unknown): CompactionResult {
  if (!value || typeof value !== "object") {
    throw new LatLngError("invalid compaction response");
  }
  return value as CompactionResult;
}

export function parseDeleted(value: unknown): boolean {
  if (!value || typeof value !== "object" || !("deleted" in value)) {
    throw new LatLngError("invalid delete response");
  }
  return Boolean((value as { deleted?: unknown }).deleted);
}

export function parseDeletedCount(value: unknown): number {
  if (!value || typeof value !== "object" || !("deleted" in value)) {
    throw new LatLngError("invalid delete response");
  }
  return Number((value as { deleted?: unknown }).deleted ?? 0);
}

export function parseStored(value: unknown): boolean {
  if (!value || typeof value !== "object" || !("stored" in value)) {
    throw new LatLngError("invalid set response");
  }
  return Boolean((value as { stored?: unknown }).stored);
}

export function parseUpdated(value: unknown): boolean {
  if (!value || typeof value !== "object" || !("updated" in value)) {
    throw new LatLngError("invalid update response");
  }
  return Boolean((value as { updated?: unknown }).updated);
}

export function parseRenamed(value: unknown): boolean {
  if (!value || typeof value !== "object" || !("renamed" in value)) {
    throw new LatLngError("invalid rename response");
  }
  return Boolean((value as { renamed?: unknown }).renamed);
}

export function parseDropped(value: unknown): boolean {
  if (!value || typeof value !== "object" || !("dropped" in value)) {
    throw new LatLngError("invalid drop response");
  }
  return Boolean((value as { dropped?: unknown }).dropped);
}

export function parseFieldValueResponse(value: unknown): FieldValue | null {
  if (value === null) {
    return null;
  }
  if (value && typeof value === "object" && "type" in value) {
    return fromWireFieldValue(value);
  }
  if (value && typeof value === "object" && "value" in value) {
    const inner = (value as { value?: unknown }).value;
    return inner === null || inner === undefined ? null : fromWireFieldValue(inner);
  }
  return fromWireFieldValue(value);
}

export function parseTtl(value: unknown): number | null {
  if (!value || typeof value !== "object" || !("ttl" in value)) {
    throw new LatLngError("invalid ttl response");
  }
  const ttl = (value as { ttl?: unknown }).ttl;
  return typeof ttl === "number" ? ttl : null;
}

export function parseJsonValue(value: unknown): unknown {
  if (value === null) {
    return null;
  }
  const inner =
    value && typeof value === "object" && "value" in value
      ? (value as { value?: unknown }).value
      : value;
  if (inner === null || inner === undefined) {
    return null;
  }
  if (typeof inner !== "string") {
    return inner;
  }
  try {
    return JSON.parse(inner) as unknown;
  } catch {
    return inner;
  }
}

export function parseBooleanResult(value: unknown, key: string): boolean {
  if (!value || typeof value !== "object" || !(key in value)) {
    throw new LatLngError(`invalid ${key} response`);
  }
  return Boolean((value as Record<string, unknown>)[key]);
}

export function parseSeconds(value: unknown): number | null {
  if (!value || typeof value !== "object" || !("seconds" in value)) {
    throw new LatLngError("invalid timeout response");
  }
  const seconds = (value as { seconds?: unknown }).seconds;
  return typeof seconds === "number" ? seconds : null;
}

export function parseChannelDefinition(value: unknown): ChannelDef {
  return parseChannelDef(value);
}

export function parseHookDefinition(value: unknown): HookDef {
  return parseHookDef(value);
}

export function parseHookSummaries(value: unknown): HookInfo[] {
  return parseHookInfoList(value);
}
