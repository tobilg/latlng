import { invariant, isRecord } from "../internal/assert.js";
import type { Area, FieldMap, GeoObject, SearchOptions } from "./models.js";
import {
  fromWireArea,
  fromWireFieldMap,
  fromWireGeoObject,
  toWireArea,
  toWireSearchOptions,
} from "./models.js";

/** Mutation command names used by geofence events and definitions. */
export type MutationCommand = "set" | "del" | "drop" | "fset";
/** Geofence detection types. */
export type DetectType =
  | "inside"
  | "outside"
  | "enter"
  | "exit"
  | "cross"
  | "roam";

/** Geofence query definition used by channels and hooks. */
export type GeofenceQuery =
  | {
      type: "nearby";
      lat: number;
      lon: number;
      meters: number;
      options?: SearchOptions;
    }
  | { type: "within"; area: Area; options?: SearchOptions }
  | { type: "intersects"; area: Area; options?: SearchOptions }
  | {
      type: "roam";
      targetCollection: string;
      targetPattern: string;
      meters: number;
      options?: SearchOptions;
      nodwell?: boolean;
    };

/** Full geofence definition shared by hooks and channels. */
export interface GeofenceDef {
  /** Target collection evaluated by the geofence. */
  collection: string;
  /** Query definition used to select matching objects. */
  query: GeofenceQuery;
  /** Detection modes emitted by the geofence. */
  detect: DetectType[];
  /** Mutation commands that should trigger the geofence. */
  commands: MutationCommand[];
}

/** Hook summary returned by list operations. */
export interface HookInfo {
  name: string;
  endpoint: string;
  collection: string;
}

/** Full channel definition returned by `getChannel`. */
export interface ChannelDef {
  name: string;
  def: GeofenceDef;
}

/** Full hook definition returned by `getHook`. */
export interface HookDef {
  name: string;
  endpoint: string;
  def: GeofenceDef;
}

/** Roaming metadata attached to roam events. */
export interface RoamingInfo {
  collection: string;
  id: string;
  meters: number;
}

/** Geofence event emitted over channels and WebSocket subscriptions. */
export interface GeofenceEvent {
  command: MutationCommand;
  detect: DetectType;
  collection: string;
  id: string;
  object: GeoObject;
  fields: FieldMap;
  timestamp_ns: number;
  /** Stable opaque event identifier for external idempotency. */
  event_id?: string | null;
  /** Stable opaque webhook delivery job identifier. */
  job_id?: string | null;
  hook?: string | null;
  group?: string | null;
  nearby?: RoamingInfo | null;
}

function commandToWire(value: MutationCommand): string {
  switch (value) {
    case "set":
      return "Set";
    case "del":
      return "Del";
    case "drop":
      return "Drop";
    case "fset":
      return "Fset";
  }
}

function detectToWire(value: DetectType): string {
  switch (value) {
    case "inside":
      return "Inside";
    case "outside":
      return "Outside";
    case "enter":
      return "Enter";
    case "exit":
      return "Exit";
    case "cross":
      return "Cross";
    case "roam":
      return "Roam";
  }
}

function commandFromWire(value: unknown): MutationCommand {
  switch (value) {
    case "Set":
    case "set":
      return "set";
    case "Del":
    case "del":
      return "del";
    case "Drop":
    case "drop":
      return "drop";
    case "Fset":
    case "fset":
      return "fset";
    default:
      throw new Error(`unknown mutation command: ${String(value)}`);
  }
}

function detectFromWire(value: unknown): DetectType {
  switch (value) {
    case "Inside":
    case "inside":
      return "inside";
    case "Outside":
    case "outside":
      return "outside";
    case "Enter":
    case "enter":
      return "enter";
    case "Exit":
    case "exit":
      return "exit";
    case "Cross":
    case "cross":
      return "cross";
    case "Roam":
    case "roam":
      return "roam";
    default:
      throw new Error(`unknown detect type: ${String(value)}`);
  }
}

/**
 * Converts a geofence query into the server wire format.
 *
 * @param value Geofence query definition.
 * @returns Wire-format geofence query.
 */
export function toWireGeofenceQuery(value: GeofenceQuery): unknown {
  switch (value.type) {
    case "nearby":
      return {
        Nearby: {
          lat: value.lat,
          lon: value.lon,
          meters: value.meters,
          options: toWireSearchOptions(value.options),
        },
      };
    case "within":
      return {
        Within: {
          area: toWireArea(value.area),
          options: toWireSearchOptions(value.options),
        },
      };
    case "intersects":
      return {
        Intersects: {
          area: toWireArea(value.area),
          options: toWireSearchOptions(value.options),
        },
      };
    case "roam":
      return {
        Roam: {
          target_collection: value.targetCollection,
          target_pattern: value.targetPattern,
          meters: value.meters,
          options: toWireSearchOptions(value.options),
          nodwell: value.nodwell ?? false,
        },
      };
  }
}

/**
 * Converts a geofence definition into the server wire format.
 *
 * @param value Geofence definition.
 * @returns Wire-format geofence definition.
 */
export function toWireGeofenceDef(value: GeofenceDef): unknown {
  return {
    collection: value.collection,
    query: toWireGeofenceQuery(value.query),
    detect: value.detect.map(detectToWire),
    commands: value.commands.map(commandToWire),
  };
}

/**
 * Parses a list of hook summaries from the server.
 *
 * @param value Raw response value.
 * @returns Parsed hook summary array.
 */
export function parseHookInfoList(value: unknown): HookInfo[] {
  invariant(Array.isArray(value), "hooks response must be an array");
  return value.map((entry) => {
    invariant(isRecord(entry), "hook info must be an object");
    invariant(typeof entry.name === "string", "hook name must be a string");
    invariant(
      typeof entry.endpoint === "string",
      "hook endpoint must be a string",
    );
    invariant(
      typeof entry.collection === "string",
      "hook collection must be a string",
    );
    return {
      name: entry.name,
      endpoint: entry.endpoint,
      collection: entry.collection,
    };
  });
}

/**
 * Parses a full channel definition from the server.
 *
 * @param value Raw response value.
 * @returns Parsed channel definition.
 */
export function parseChannelDef(value: unknown): ChannelDef {
  invariant(isRecord(value), "channel definition must be an object");
  invariant(typeof value.name === "string", "channel name must be a string");
  return {
    name: value.name,
    def: parseGeofenceDef(value.def),
  };
}

/**
 * Parses a full hook definition from the server.
 *
 * @param value Raw response value.
 * @returns Parsed hook definition.
 */
export function parseHookDef(value: unknown): HookDef {
  invariant(isRecord(value), "hook definition must be an object");
  invariant(typeof value.name === "string", "hook name must be a string");
  invariant(typeof value.endpoint === "string", "hook endpoint must be a string");
  return {
    name: value.name,
    endpoint: value.endpoint,
    def: parseGeofenceDef(value.def),
  };
}

/**
 * Parses a geofence event from a WebSocket or channel payload.
 *
 * @param value Raw event value.
 * @returns Parsed geofence event.
 */
export function parseGeofenceEvent(value: unknown): GeofenceEvent {
  invariant(isRecord(value), "geofence event must be an object");
  invariant(typeof value.collection === "string", "event collection missing");
  invariant(typeof value.id === "string", "event id missing");
  invariant(typeof value.timestamp_ns === "number", "timestamp missing");
  return {
    command: commandFromWire(value.command),
    detect: detectFromWire(value.detect),
    collection: value.collection,
    id: value.id,
    object: fromWireGeoObject(value.object),
    fields: fromWireFieldMap(value.fields),
    timestamp_ns: value.timestamp_ns,
    event_id: typeof value.event_id === "string" ? value.event_id : null,
    job_id: typeof value.job_id === "string" ? value.job_id : null,
    hook: typeof value.hook === "string" ? value.hook : null,
    group: typeof value.group === "string" ? value.group : null,
    nearby:
      isRecord(value.nearby) &&
      typeof value.nearby.collection === "string" &&
      typeof value.nearby.id === "string" &&
      typeof value.nearby.meters === "number"
        ? {
            collection: value.nearby.collection,
            id: value.nearby.id,
            meters: value.nearby.meters,
          }
        : null,
  };
}

function parseGeofenceDef(value: unknown): GeofenceDef {
  invariant(isRecord(value), "geofence definition must be an object");
  invariant(
    typeof value.collection === "string",
    "geofence collection must be a string",
  );
  invariant(Array.isArray(value.detect), "geofence detect must be an array");
  invariant(Array.isArray(value.commands), "geofence commands must be an array");
  return {
    collection: value.collection,
    query: parseGeofenceQuery(value.query),
    detect: value.detect.map(detectFromWire),
    commands: value.commands.map(commandFromWire),
  };
}

function parseGeofenceQuery(value: unknown): GeofenceQuery {
  invariant(isRecord(value), "geofence query must be an object");
  if ("Nearby" in value) {
    const nearby = value.Nearby;
    invariant(isRecord(nearby), "Nearby payload must be an object");
    invariant(typeof nearby.lat === "number", "nearby lat must be a number");
    invariant(typeof nearby.lon === "number", "nearby lon must be a number");
    invariant(
      typeof nearby.meters === "number",
      "nearby meters must be a number",
    );
    return {
      type: "nearby",
      lat: nearby.lat,
      lon: nearby.lon,
      meters: nearby.meters,
      options: parseSearchOptions(nearby.options),
    };
  }
  if ("Within" in value) {
    const within = value.Within;
    invariant(isRecord(within), "Within payload must be an object");
    return {
      type: "within",
      area: fromWireArea(within.area),
      options: parseSearchOptions(within.options),
    };
  }
  if ("Intersects" in value) {
    const intersects = value.Intersects;
    invariant(isRecord(intersects), "Intersects payload must be an object");
    return {
      type: "intersects",
      area: fromWireArea(intersects.area),
      options: parseSearchOptions(intersects.options),
    };
  }
  if ("Roam" in value) {
    const roam = value.Roam;
    invariant(isRecord(roam), "Roam payload must be an object");
    invariant(
      typeof roam.target_collection === "string",
      "roam target_collection must be a string",
    );
    invariant(
      typeof roam.target_pattern === "string",
      "roam target_pattern must be a string",
    );
    invariant(typeof roam.meters === "number", "roam meters must be a number");
    return {
      type: "roam",
      targetCollection: roam.target_collection,
      targetPattern: roam.target_pattern,
      meters: roam.meters,
      options: parseSearchOptions(roam.options),
      nodwell: typeof roam.nodwell === "boolean" ? roam.nodwell : undefined,
    };
  }
  throw new Error("unknown geofence query shape");
}

function parseSearchOptions(value: unknown): SearchOptions | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const limit = typeof value.limit === "number" ? value.limit : undefined;
  const cursor = typeof value.cursor === "number" ? value.cursor : undefined;
  const nofields =
    typeof value.nofields === "boolean" ? value.nofields : undefined;
  const matchPattern =
    typeof value.match_pattern === "string" ? value.match_pattern : undefined;
  const clip = typeof value.clip === "boolean" ? value.clip : undefined;
  let sort: SearchOptions["sort"];
  if (value.sort === "Desc") {
    sort = "desc";
  } else if (value.sort === "Asc") {
    sort = "asc";
  }
  return {
    cursor,
    limit,
    nofields,
    matchPattern,
    sort,
    clip,
  };
}
