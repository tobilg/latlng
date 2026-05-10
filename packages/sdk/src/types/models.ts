import { invariant, isRecord } from "../internal/assert.js";
import type { JsonValue } from "./json.js";

/** Geographic bounding box expressed as min/max latitude and longitude. */
export interface BoundingBox {
  /** Southern latitude bound. */
  min_lat: number;
  /** Western longitude bound. */
  min_lon: number;
  /** Northern latitude bound. */
  max_lat: number;
  /** Eastern longitude bound. */
  max_lon: number;
}

/** Field value stored alongside an object. */
export type FieldValue =
  | { type: "number"; value: number }
  | { type: "text"; value: string }
  | { type: "json"; value: string };

/** Named field assignment. */
export interface FieldEntry {
  /** Field name. */
  name: string;
  /** Field value. */
  value: FieldValue;
}

/** Field map keyed by field name. */
export type FieldMap = Record<string, FieldValue>;

/** Stored geographic object representation. */
export type GeoObject =
  | { type: "point"; lat: number; lon: number; z?: number | null }
  | { type: "bounds"; bounds: BoundingBox }
  | { type: "hash"; value: string }
  | { type: "geojson"; value: JsonValue }
  | { type: "string"; value: string };

/** Object returned by the `latlng` server. */
export interface LatLngObject {
  /** Object identifier. */
  id: string;
  /** Stored geometry or string payload. */
  geo: GeoObject;
  /** Attached fields. */
  fields: FieldMap;
  /** Expiration timestamp, when set. */
  expires_at: number | null;
}

/** Area definition used by spatial query APIs. */
export type Area =
  | { type: "circle"; lat: number; lon: number; meters: number }
  | { type: "bounds"; bounds: BoundingBox }
  | { type: "hash"; value: string }
  | { type: "geojson"; value: JsonValue }
  | { type: "tile"; x: number; y: number; z: number }
  | { type: "quadkey"; value: string }
  | {
      type: "sector";
      lat: number;
      lon: number;
      meters: number;
      bearing1: number;
      bearing2: number;
    }
  | { type: "reference"; collection: string; id: string };

/** Sort direction for search results. */
export type SortOrder = "asc" | "desc";

/** Output format requested from spatial query endpoints. */
export type OutputFormat =
  | "objects"
  | "points"
  | "bounds"
  | "ids"
  | "count"
  | { hashes: { precision: number } };

/** Numeric or textual comparison used in `where` clauses. */
export type WhereComparison =
  | { type: "range"; min: number; max: number }
  | { type: "equalsText"; value: string }
  | { type: "regex"; value: string };

/** Field-level filter used by search endpoints. */
export interface WhereFilter {
  /** Field name to compare. */
  field: string;
  /** Comparison to apply to the field. */
  comparison: WhereComparison;
}

/** Inclusion filter used by search endpoints. */
export interface WhereInFilter {
  /** Field name to inspect. */
  field: string;
  /** Accepted string values. */
  values: string[];
}

/** Expression filter used by search endpoints. */
export interface WhereExprFilter {
  /** Server-side expression string. */
  expression: string;
}

/** Shared options accepted by search endpoints. */
export interface SearchOptions {
  /** Cursor to resume a paginated query. */
  cursor?: number;
  /** Maximum number of results to return. */
  limit?: number;
  /** Omit field maps from result items when `true`. */
  nofields?: boolean;
  /** Match pattern applied to object IDs. */
  matchPattern?: string;
  /** Result sort direction. */
  sort?: SortOrder;
  /** Numeric/text `where` filters. */
  whereFilters?: WhereFilter[];
  /** `where in` filters. */
  whereInFilters?: WhereInFilter[];
  /** Expression filters. */
  whereExprFilters?: WhereExprFilter[];
  /** Clip geometries to the query area when supported. */
  clip?: boolean;
  /** Output format for result items. */
  output?: OutputFormat;
}

/** Request payload for `nearby` searches. */
export interface NearbyQuery {
  /** Center latitude. */
  lat: number;
  /** Center longitude. */
  lon: number;
  /** Radius in meters. */
  meters: number;
  /** Additional search options. */
  options?: SearchOptions;
}

/** Single result item returned by a search. */
export interface SearchItem {
  /** Object identifier. */
  id: string;
  /** Returned object when requested by the output format. */
  object?: GeoObject;
  /** Returned fields when requested. */
  fields?: FieldMap;
  /** Distance from the query origin, when relevant. */
  distance_meters?: number | null;
}

/** Search response payload. */
export interface SearchResults {
  /** Result items for the current page. */
  results: SearchItem[];
  /** Cursor for the next page. */
  cursor: number;
  /** Total result count or count for the requested page, depending on the server output mode. */
  count: number;
}

/** Collection statistics returned by the server. */
export interface CollectionStats {
  /** Collection name. */
  name: string;
  /** Number of stored objects. */
  object_count: number;
  /** Number of point objects. */
  point_count: number;
  /** Number of string objects. */
  string_count: number;
  /** Number of objects with TTLs. */
  expires_count: number;
}

/** Collection metadata returned by `getCollection`. */
export interface CollectionInfo {
  /** Collection name. */
  name: string;
  /** Collection bounds when available. */
  bounds: BoundingBox | null;
  /** Collection statistics. */
  stats: CollectionStats;
}

/** AOF compaction before/after statistics. */
export interface CompactionResult {
  before_entries: number;
  after_entries: number;
  before_bytes: number;
  after_bytes: number;
}

/** Server status returned by the `/server` endpoint. */
export interface ServerInfo {
  version: string;
  api_version: string;
  protocol_version: string;
  storage_format_version: string;
  num_collections: number;
  num_objects: number;
  num_points: number;
  heap_bytes: number;
  read_only: boolean;
  leader: boolean;
  server_id: string;
  following: string | null;
  caught_up: boolean;
  caught_up_once: boolean;
  last_sequence: number;
}

/**
 * Creates a point object payload.
 *
 * @param lat Latitude.
 * @param lon Longitude.
 * @param z Optional elevation component.
 * @returns Point object payload.
 */
export function point(lat: number, lon: number, z?: number | null): GeoObject {
  return { type: "point", lat, lon, z };
}

/**
 * Creates a bounds object payload.
 *
 * @param boundsValue Bounding box value.
 * @returns Bounds object payload.
 */
export function bounds(boundsValue: BoundingBox): GeoObject {
  return { type: "bounds", bounds: boundsValue };
}

/**
 * Creates a geohash object payload.
 *
 * @param value Hash string.
 * @returns Hash object payload.
 */
export function hash(value: string): GeoObject {
  return { type: "hash", value };
}

/**
 * Creates a GeoJSON object payload.
 *
 * @param value GeoJSON value.
 * @returns GeoJSON object payload.
 */
export function geojson(value: JsonValue): GeoObject {
  return { type: "geojson", value };
}

/**
 * Creates a string object payload.
 *
 * @param value String value.
 * @returns String object payload.
 */
export function stringObject(value: string): GeoObject {
  return { type: "string", value };
}

/**
 * Creates a numeric field value.
 *
 * @param value Numeric field value.
 * @returns Field value payload.
 */
export function numberField(value: number): FieldValue {
  return { type: "number", value };
}

/**
 * Creates a text field value.
 *
 * @param value Text field value.
 * @returns Field value payload.
 */
export function textField(value: string): FieldValue {
  return { type: "text", value };
}

/**
 * Creates a JSON field value.
 *
 * @param value JSON string value.
 * @returns Field value payload.
 */
export function jsonField(value: string): FieldValue {
  return { type: "json", value };
}

/**
 * Converts a field map into an ordered list of field entries.
 *
 * @param fields Field map.
 * @returns Field entry array.
 */
export function fieldEntries(fields: FieldMap | undefined): FieldEntry[] {
  return Object.entries(fields ?? {}).map(([name, value]) => ({ name, value }));
}

/**
 * Converts a field value into the server wire format.
 *
 * @param value Field value.
 * @returns Wire-format field value.
 */
export function toWireFieldValue(value: FieldValue): unknown {
  switch (value.type) {
    case "number":
      return { type: "number", value: value.value };
    case "text":
      return { type: "text", value: value.value };
    case "json":
      return { type: "json", value: value.value };
  }
}

/**
 * Parses a field value from the server wire format.
 *
 * @param value Raw response value.
 * @returns Parsed field value.
 */
export function fromWireFieldValue(value: unknown): FieldValue {
  invariant(isRecord(value), "field value must be an object");
  const type = value.type;
  const inner = value.value;
  invariant(typeof type === "string", "field value type must be a string");
  if (type === "number") {
    invariant(typeof inner === "number", "field number value must be a number");
    return { type, value: inner };
  }
  invariant(typeof inner === "string", "field text/json value must be a string");
  if (type === "text" || type === "json") {
    return { type, value: inner };
  }
  throw new Error(`unknown field value type: ${type}`);
}

/**
 * Converts field entries into the server wire format.
 *
 * @param fields Field map or entry list.
 * @returns Wire-format field entry array.
 */
export function toWireFieldEntries(fields: FieldEntry[] | FieldMap | undefined): unknown[] {
  if (!fields) {
    return [];
  }
  const entries = Array.isArray(fields) ? fields : fieldEntries(fields);
  return entries.map((entry) => ({
    name: entry.name,
    value: toWireFieldValue(entry.value),
  }));
}

/**
 * Parses a field map from the server wire format.
 *
 * @param value Raw response value.
 * @returns Parsed field map.
 */
export function fromWireFieldMap(value: unknown): FieldMap {
  if (!isRecord(value)) {
    return {};
  }
  const values = value.values;
  if (!isRecord(values)) {
    return {};
  }
  return Object.fromEntries(
    Object.entries(values).map(([key, inner]) => [key, fromWireFieldValue(inner)]),
  );
}

/**
 * Converts a geographic object into the server wire format.
 *
 * @param value Geographic object.
 * @returns Wire-format object payload.
 */
export function toWireGeoObject(value: GeoObject): unknown {
  switch (value.type) {
    case "point":
      return { Point: { lat: value.lat, lon: value.lon, z: value.z ?? null } };
    case "bounds":
      return { Bounds: value.bounds };
    case "hash":
      return { Hash: value.value };
    case "geojson":
      return { GeoJson: value.value };
    case "string":
      return { String: value.value };
  }
}

/**
 * Parses a geographic object from the server wire format.
 *
 * @param value Raw response value.
 * @returns Parsed geographic object.
 */
export function fromWireGeoObject(value: unknown): GeoObject {
  invariant(isRecord(value), "geo object must be an object");
  if ("Point" in value) {
    const pointValue = value.Point;
    invariant(isRecord(pointValue), "Point payload must be an object");
    invariant(typeof pointValue.lat === "number", "point lat must be a number");
    invariant(typeof pointValue.lon === "number", "point lon must be a number");
    return {
      type: "point",
      lat: pointValue.lat,
      lon: pointValue.lon,
      z:
        typeof pointValue.z === "number" || pointValue.z === null
          ? pointValue.z
          : undefined,
    };
  }
  if ("Bounds" in value) {
    const boundsValue = value.Bounds;
    invariant(isRecord(boundsValue), "Bounds payload must be an object");
    return {
      type: "bounds",
      bounds: {
        min_lat: Number(boundsValue.min_lat),
        min_lon: Number(boundsValue.min_lon),
        max_lat: Number(boundsValue.max_lat),
        max_lon: Number(boundsValue.max_lon),
      },
    };
  }
  if ("Hash" in value) {
    invariant(typeof value.Hash === "string", "Hash payload must be a string");
    return { type: "hash", value: value.Hash };
  }
  if ("GeoJson" in value) {
    return { type: "geojson", value: value.GeoJson as JsonValue };
  }
  if ("String" in value) {
    invariant(typeof value.String === "string", "String payload must be a string");
    return { type: "string", value: value.String };
  }
  throw new Error("unknown geo object shape");
}

/**
 * Converts an area definition into the server wire format.
 *
 * @param area Area definition.
 * @returns Wire-format area payload.
 */
export function toWireArea(area: Area): unknown {
  switch (area.type) {
    case "circle":
      return { Circle: { lat: area.lat, lon: area.lon, meters: area.meters } };
    case "bounds":
      return { Bounds: area.bounds };
    case "hash":
      return { Hash: area.value };
    case "geojson":
      return { GeoJson: area.value };
    case "tile":
      return { Tile: { x: area.x, y: area.y, z: area.z } };
    case "quadkey":
      return { Quadkey: area.value };
    case "sector":
      return {
        Sector: {
          lat: area.lat,
          lon: area.lon,
          meters: area.meters,
          bearing1: area.bearing1,
          bearing2: area.bearing2,
        },
      };
    case "reference":
      return { Reference: { collection: area.collection, id: area.id } };
  }
}

/**
 * Parses an area definition from the server wire format.
 *
 * @param value Raw response value.
 * @returns Parsed area definition.
 */
export function fromWireArea(value: unknown): Area {
  invariant(isRecord(value), "area must be an object");
  if ("Circle" in value) {
    const circleValue = value.Circle;
    invariant(isRecord(circleValue), "Circle payload must be an object");
    invariant(typeof circleValue.lat === "number", "circle lat must be a number");
    invariant(typeof circleValue.lon === "number", "circle lon must be a number");
    invariant(
      typeof circleValue.meters === "number",
      "circle meters must be a number",
    );
    return {
      type: "circle",
      lat: circleValue.lat,
      lon: circleValue.lon,
      meters: circleValue.meters,
    };
  }
  if ("Bounds" in value) {
    const boundsValue = value.Bounds;
    invariant(isRecord(boundsValue), "Bounds payload must be an object");
    return {
      type: "bounds",
      bounds: {
        min_lat: Number(boundsValue.min_lat),
        min_lon: Number(boundsValue.min_lon),
        max_lat: Number(boundsValue.max_lat),
        max_lon: Number(boundsValue.max_lon),
      },
    };
  }
  if ("Hash" in value) {
    invariant(typeof value.Hash === "string", "Hash payload must be a string");
    return { type: "hash", value: value.Hash };
  }
  if ("GeoJson" in value) {
    return { type: "geojson", value: value.GeoJson as JsonValue };
  }
  if ("Tile" in value) {
    const tileValue = value.Tile;
    invariant(isRecord(tileValue), "Tile payload must be an object");
    invariant(typeof tileValue.x === "number", "tile x must be a number");
    invariant(typeof tileValue.y === "number", "tile y must be a number");
    invariant(typeof tileValue.z === "number", "tile z must be a number");
    return {
      type: "tile",
      x: tileValue.x,
      y: tileValue.y,
      z: tileValue.z,
    };
  }
  if ("Quadkey" in value) {
    invariant(
      typeof value.Quadkey === "string",
      "Quadkey payload must be a string",
    );
    return { type: "quadkey", value: value.Quadkey };
  }
  if ("Sector" in value) {
    const sectorValue = value.Sector;
    invariant(isRecord(sectorValue), "Sector payload must be an object");
    invariant(typeof sectorValue.lat === "number", "sector lat must be a number");
    invariant(typeof sectorValue.lon === "number", "sector lon must be a number");
    invariant(
      typeof sectorValue.meters === "number",
      "sector meters must be a number",
    );
    invariant(
      typeof sectorValue.bearing1 === "number",
      "sector bearing1 must be a number",
    );
    invariant(
      typeof sectorValue.bearing2 === "number",
      "sector bearing2 must be a number",
    );
    return {
      type: "sector",
      lat: sectorValue.lat,
      lon: sectorValue.lon,
      meters: sectorValue.meters,
      bearing1: sectorValue.bearing1,
      bearing2: sectorValue.bearing2,
    };
  }
  if ("Reference" in value) {
    const referenceValue = value.Reference;
    invariant(isRecord(referenceValue), "Reference payload must be an object");
    invariant(
      typeof referenceValue.collection === "string",
      "reference collection must be a string",
    );
    invariant(typeof referenceValue.id === "string", "reference id must be a string");
    return {
      type: "reference",
      collection: referenceValue.collection,
      id: referenceValue.id,
    };
  }
  throw new Error("unknown area shape");
}

/**
 * Converts a sort order into the server wire format.
 *
 * @param value Sort order.
 * @returns Wire-format sort order.
 */
export function toWireSortOrder(value: SortOrder | undefined): unknown {
  if (value === "desc") {
    return "Desc";
  }
  return "Asc";
}

/**
 * Converts an output format into the server wire format.
 *
 * @param value Output format.
 * @returns Wire-format output format.
 */
export function toWireOutputFormat(value: OutputFormat | undefined): unknown {
  if (!value || value === "objects") {
    return "Objects";
  }
  if (value === "points") {
    return "Points";
  }
  if (value === "bounds") {
    return "Bounds";
  }
  if (value === "ids") {
    return "Ids";
  }
  if (value === "count") {
    return "Count";
  }
  return { Hashes: { precision: value.hashes.precision } };
}

/**
 * Converts a `where` comparison into the server wire format.
 *
 * @param value Comparison value.
 * @returns Wire-format comparison.
 */
export function toWireWhereComparison(value: WhereComparison): unknown {
  switch (value.type) {
    case "range":
      return { Range: { min: value.min, max: value.max } };
    case "equalsText":
      return { EqualsText: value.value };
    case "regex":
      return { Regex: value.value };
  }
}

/**
 * Converts search options into the server wire format.
 *
 * @param options Search options.
 * @returns Wire-format search options.
 */
export function toWireSearchOptions(options: SearchOptions | undefined): unknown {
  const value = options ?? {};
  return {
    cursor: value.cursor ?? 0,
    limit: value.limit ?? 100,
    nofields: value.nofields ?? false,
    match_pattern: value.matchPattern ?? null,
    sort: toWireSortOrder(value.sort),
    where_filters: (value.whereFilters ?? []).map((filter) => ({
      field: filter.field,
      comparison: toWireWhereComparison(filter.comparison),
    })),
    where_in_filters: value.whereInFilters ?? [],
    where_expr_filters: value.whereExprFilters ?? [],
    clip: value.clip ?? false,
    output: toWireOutputFormat(value.output),
  };
}

/**
 * Parses an object response from the server.
 *
 * @param value Raw response value.
 * @returns Parsed object or `null`.
 */
export function fromWireObject(value: unknown): LatLngObject | null {
  if (value === null || value === undefined) {
    return null;
  }
  invariant(isRecord(value), "object response must be an object");
  invariant(typeof value.id === "string", "object id must be a string");
  return {
    id: value.id,
    geo: fromWireGeoObject(value.geo),
    fields: fromWireFieldMap(value.fields),
    expires_at:
      typeof value.expires_at === "number" ? value.expires_at : null,
  };
}

/**
 * Parses search results from the server wire format.
 *
 * @param value Raw response value.
 * @returns Parsed search results.
 */
export function fromWireSearchResults(value: unknown): SearchResults {
  invariant(isRecord(value), "search results must be an object");
  invariant(Array.isArray(value.results), "search results array is missing");
  return {
    results: value.results.map((entry) => {
      invariant(isRecord(entry), "search result item must be an object");
      invariant(typeof entry.id === "string", "search result id must be a string");
      return {
        id: entry.id,
        object:
          entry.object === null || entry.object === undefined
            ? undefined
            : fromWireGeoObject(entry.object),
        fields:
          entry.fields === null || entry.fields === undefined
            ? undefined
            : fromWireFieldMap(entry.fields),
        distance_meters:
          typeof entry.distance_meters === "number"
            ? entry.distance_meters
            : null,
      };
    }),
    cursor: Number(value.cursor ?? 0),
    count: Number(value.count ?? 0),
  };
}

/**
 * Parses the `/server` endpoint response.
 *
 * @param value Raw response value.
 * @returns Parsed server info.
 */
export function parseServerInfo(value: unknown): ServerInfo {
  invariant(isRecord(value), "server info must be an object");
  return {
    version: typeof value.version === "string" ? value.version : "",
    api_version: typeof value.api_version === "string" ? value.api_version : "",
    protocol_version:
      typeof value.protocol_version === "string" ? value.protocol_version : "",
    storage_format_version:
      typeof value.storage_format_version === "string"
        ? value.storage_format_version
        : "",
    num_collections: Number(value.num_collections ?? 0),
    num_objects: Number(value.num_objects ?? 0),
    num_points: Number(value.num_points ?? 0),
    heap_bytes: Number(value.heap_bytes ?? 0),
    read_only: Boolean(value.read_only),
    leader: Boolean(value.leader),
    server_id: typeof value.server_id === "string" ? value.server_id : "",
    following:
      typeof value.following === "string" ? value.following : null,
    caught_up: Boolean(value.caught_up),
    caught_up_once: Boolean(value.caught_up_once),
    last_sequence: Number(value.last_sequence ?? 0),
  };
}
