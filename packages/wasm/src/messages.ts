import type { GeofenceEvent, MutationEvent } from "./types.js";

export type WorkerMethod =
  | "init"
  | "createCollection"
  | "dropCollection"
  | "collections"
  | "collectionInfo"
  | "setObject"
  | "getObject"
  | "deleteObject"
  | "expire"
  | "persist"
  | "ttl"
  | "setHook"
  | "deleteHook"
  | "hooks"
  | "getHook"
  | "nearby"
  | "within"
  | "intersects"
  | "scan"
  | "search"
  | "bounds"
  | "stats"
  | "serverInfo";

export interface WorkerRequest {
  id: number;
  method: WorkerMethod;
  params: unknown[];
}

export type WorkerResponse =
  | { id: number; ok: true; result: unknown }
  | { id: number; ok: false; error: string };

export type WorkerEventMessage =
  | { type: "event"; eventType: "mutation"; event: MutationEvent }
  | { type: "event"; eventType: "geofence"; event: GeofenceEvent };

export type WorkerOutboundMessage = WorkerResponse | WorkerEventMessage;

export interface WasmMutationResponse<T = unknown> {
  ok: boolean;
  result: T;
  events: unknown[];
}
