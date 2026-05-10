/** JSON value accepted by GeoJSON payloads and field values. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

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

/** Stored field value. */
export type FieldValue =
  | { type: "number"; value: number }
  | { type: "text"; value: string }
  | { type: "json"; value: JsonValue };

/** Named field assignment. */
export interface FieldEntry {
  /** Field name. */
  name: string;
  /** Field value. */
  value: FieldValue;
}

/** Field map keyed by field name. */
export type FieldMap = Record<string, FieldValue>;

/** Geographic object payload accepted by `setObject`. */
export type GeoObject =
  | { type: "point"; lat: number; lon: number; z?: number | null }
  | { type: "bounds"; bounds: BoundingBox }
  | { type: "hash"; value: string }
  | { type: "geojson"; value: JsonValue }
  | { type: "string"; value: string };

/** Stored object returned by read and query operations. */
export interface LatLngObject {
  /** Object identifier. */
  id: string;
  /** Stored object payload. */
  geo: GeoObject;
  /** Stored field values. */
  fields: FieldMap;
  /** Expiration timestamp in milliseconds since epoch, or `null`. */
  expires_at: number | null;
}

/** Area definition used by spatial queries and geofence definitions. */
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

/** Sort direction for query results. */
export type SortOrder = "asc" | "desc";

/** Query output format. */
export type OutputFormat =
  | "objects"
  | "points"
  | "bounds"
  | "ids"
  | "count"
  | { hashes: { precision: number } };

/** Numeric or textual field comparison. */
export type WhereComparison =
  | { type: "range"; min: number; max: number }
  | { type: "equalsText"; value: string }
  | { type: "regex"; value: string };

/** Field-level filter used by search queries. */
export interface WhereFilter {
  /** Field name to inspect. */
  field: string;
  /** Comparison to apply. */
  comparison: WhereComparison;
}

/** Inclusion filter used by search queries. */
export interface WhereInFilter {
  /** Field name to inspect. */
  field: string;
  /** Accepted string values. */
  values: string[];
}

/** Expression filter used by search queries. */
export interface WhereExprFilter {
  /** Server-side expression string. */
  expression: string;
}

/** Shared options accepted by scan/search/spatial query methods. */
export interface SearchOptions {
  /** Cursor to resume a paginated query. */
  cursor?: number;
  /** Maximum number of results to return. */
  limit?: number;
  /** Omit fields from result items when true. */
  nofields?: boolean;
  /** Match pattern applied to object IDs. */
  matchPattern?: string;
  /** Result sort direction. */
  sort?: SortOrder;
  /** Numeric/text filters. */
  whereFilters?: WhereFilter[];
  /** Inclusion filters. */
  whereInFilters?: WhereInFilter[];
  /** Expression filters. */
  whereExprFilters?: WhereExprFilter[];
  /** Clip geometries to the query area when supported. */
  clip?: boolean;
  /** Output format for result items. */
  output?: OutputFormat;
}

/** Nearby query request. */
export interface NearbyQuery {
  /** Center latitude. */
  lat: number;
  /** Center longitude. */
  lon: number;
  /** Search radius in meters. */
  meters: number;
  /** Optional query controls. */
  options?: SearchOptions;
}

/** Area-based query request shared by `within` and `intersects`. */
export interface AreaQuery {
  /** Area to test. */
  area: Area;
  /** Optional query controls. */
  options?: SearchOptions;
}

/** Query result item. */
export interface SearchItem {
  /** Object identifier. */
  id: string;
  /** Object payload when requested by output options. */
  object?: GeoObject;
  /** Field map unless omitted by query options. */
  fields?: FieldMap;
  /** Distance from query center for nearby queries. */
  distance_meters: number | null;
}

/** Query results returned by scan/search/spatial queries. */
export interface SearchResults {
  /** Result objects. */
  results: SearchItem[];
  /** Cursor for the next page. */
  cursor: number;
  /** Number of returned items. */
  count: number;
}

/** Collection statistics. */
export interface CollectionStats {
  /** Collection name. */
  name: string;
  /** Number of live objects. */
  object_count: number;
  /** Number of point objects. */
  point_count: number;
  /** Number of string objects. */
  string_count: number;
  /** Number of objects with expiration metadata. */
  expires_count: number;
}

/** Collection metadata returned by `collectionInfo`. */
export interface CollectionInfo {
  /** Collection name. */
  name: string;
  /** Spatial bounds, if the collection contains spatial objects. */
  bounds: BoundingBox | null;
  /** Collection statistics. */
  stats: CollectionStats;
}

/** Engine metadata returned by `serverInfo`. */
export interface ServerInfo {
  /** Product version. */
  version: string;
  /** Public API version. */
  api_version: string;
  /** Protocol version used by native transports. */
  protocol_version: string;
  /** Storage format version. */
  storage_format_version: string;
  /** Number of collections. */
  num_collections: number;
  /** Number of live objects. */
  num_objects: number;
  /** Number of live point objects. */
  num_points: number;
  /** Approximate heap usage in bytes. */
  heap_bytes: number;
  /** Whether writes are disabled. */
  read_only: boolean;
  /** Whether the engine is acting as leader. */
  leader: boolean;
  /** Current storage sequence. */
  last_sequence: number;
}

/** Object upsert options. */
export interface SetObjectOptions {
  /** Field values to store alongside the object. */
  fields?: FieldMap | FieldEntry[];
  /** Optional object TTL in seconds. */
  expireSeconds?: number;
  /** Conditional write mode. */
  condition?: "always" | "nx" | "xx";
}

/** Mutation command names used by geofence definitions and events. */
export type MutationCommand = "set" | "del" | "drop" | "fset";

/** Geofence detection mode. */
export type DetectType =
  | "inside"
  | "outside"
  | "enter"
  | "exit"
  | "cross"
  | "roam";

/** Geofence query definition. */
export type GeofenceQuery =
  | { type: "nearby"; lat: number; lon: number; meters: number; options?: SearchOptions }
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

/** Geofence hook definition. */
export interface GeofenceDef {
  /** Target collection evaluated by the hook. */
  collection: string;
  /** Query used by the hook. */
  query: GeofenceQuery;
  /** Detection modes emitted by the hook. */
  detect: DetectType[];
  /** Mutation commands that trigger the hook. */
  commands: MutationCommand[];
}

/** Hook summary. */
export interface HookInfo {
  /** Hook name. */
  name: string;
  /** Internal browser event endpoint. */
  endpoint: string;
  /** Target collection. */
  collection: string;
}

/** Full hook definition. */
export interface HookDef {
  /** Hook name. */
  name: string;
  /** Internal browser event endpoint. */
  endpoint: string;
  /** Geofence definition. */
  def: GeofenceDef;
}

/** Roaming metadata attached to roam events. */
export interface RoamingInfo {
  /** Target collection. */
  collection: string;
  /** Target object ID. */
  id: string;
  /** Roam distance in meters. */
  meters: number;
}

/** Geofence event emitted by browser hooks. */
export interface GeofenceEvent {
  /** Mutation command that triggered the event. */
  command: MutationCommand;
  /** Detection mode that matched. */
  detect: DetectType;
  /** Collection containing the changed object. */
  collection: string;
  /** Object ID. */
  id: string;
  /** Object payload at event time. */
  object: GeoObject;
  /** Field map at event time. */
  fields: FieldMap;
  /** Event timestamp in nanoseconds. */
  timestamp_ns: number;
  /** Stable opaque event identifier when available. */
  event_id?: string | null;
  /** Browser hook name when available. */
  hook?: string | null;
  /** Roaming group when available. */
  group?: string | null;
  /** Roaming metadata when available. */
  nearby?: RoamingInfo | null;
}

/** Client event emitted for object and collection mutations. */
export interface MutationEvent {
  /** Event type. */
  type:
    | "collection:create"
    | "collection:drop"
    | "object:set"
    | "object:delete"
    | "object:expire"
    | "object:persist"
    | "hook:set"
    | "hook:delete";
  /** Collection name, when applicable. */
  collection?: string;
  /** Object ID, when applicable. */
  id?: string;
  /** Hook name, when applicable. */
  hook?: string;
  /** Whether the mutation changed state. */
  changed?: boolean;
}

/** Event map used by `LatLngWasmClient.addEventListener`. */
export interface LatLngWasmEventMap {
  /** Any object, collection, or hook mutation. */
  mutation: CustomEvent<MutationEvent>;
  /** Any browser geofence hook event. */
  geofence: CustomEvent<GeofenceEvent>;
  /** Inside geofence event. */
  "geofence:inside": CustomEvent<GeofenceEvent>;
  /** Outside geofence event. */
  "geofence:outside": CustomEvent<GeofenceEvent>;
  /** Enter geofence event. */
  "geofence:enter": CustomEvent<GeofenceEvent>;
  /** Exit geofence event. */
  "geofence:exit": CustomEvent<GeofenceEvent>;
  /** Cross geofence event. */
  "geofence:cross": CustomEvent<GeofenceEvent>;
  /** Roam geofence event. */
  "geofence:roam": CustomEvent<GeofenceEvent>;
}

export function toWireSetObjectRequest(
  object: GeoObject,
  options: SetObjectOptions | undefined,
): unknown {
  return {
    object: toWireGeoObject(object),
    fields: toWireFieldEntries(options?.fields),
    expire_seconds: options?.expireSeconds ?? null,
    condition: toWireSetCondition(options?.condition),
  };
}

export function toWireNearbyQuery(query: NearbyQuery): unknown {
  return {
    lat: query.lat,
    lon: query.lon,
    meters: query.meters,
    options: toWireSearchOptions(query.options),
  };
}

export function toWireAreaQuery(query: AreaQuery): unknown {
  return {
    area: toWireArea(query.area),
    options: toWireSearchOptions(query.options),
  };
}

export function toWireSearchOptions(options: SearchOptions | undefined): unknown {
  const value = options ?? {};
  return {
    cursor: value.cursor ?? 0,
    limit: value.limit ?? 100,
    nofields: value.nofields ?? false,
    match_pattern: value.matchPattern ?? null,
    sort: value.sort === "desc" ? "Desc" : "Asc",
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

export function toWireGeofenceDef(value: GeofenceDef): unknown {
  return {
    collection: value.collection,
    query: toWireGeofenceQuery(value.query),
    detect: value.detect.map(toWireDetectType),
    commands: value.commands.map(toWireMutationCommand),
  };
}

export function fromWireObject(value: unknown): LatLngObject | null {
  if (value === null || value === undefined) {
    return null;
  }
  assertRecord(value, "object response");
  return {
    id: expectString(value.id, "object id"),
    geo: fromWireGeoObject(value.geo),
    fields: fromWireFieldMap(value.fields),
    expires_at: typeof value.expires_at === "number" ? value.expires_at : null,
  };
}

export function fromWireSearchResults(value: unknown): SearchResults {
  assertRecord(value, "search results");
  if (!Array.isArray(value.results)) {
    throw new Error("search results must include results array");
  }
  return {
    results: value.results.map((entry) => {
      assertRecord(entry, "search result item");
      return {
        id: expectString(entry.id, "result id"),
        object:
          entry.object === null || entry.object === undefined
            ? undefined
            : fromWireGeoObject(entry.object),
        fields:
          entry.fields === null || entry.fields === undefined
            ? undefined
            : fromWireFieldMap(entry.fields),
        distance_meters:
          typeof entry.distance_meters === "number" ? entry.distance_meters : null,
      };
    }),
    cursor: Number(value.cursor ?? 0),
    count: Number(value.count ?? 0),
  };
}

export function fromWireGeofenceEvent(value: unknown): GeofenceEvent {
  assertRecord(value, "geofence event");
  return {
    command: fromWireMutationCommand(value.command),
    detect: fromWireDetectType(value.detect),
    collection: expectString(value.collection, "event collection"),
    id: expectString(value.id, "event id"),
    object: fromWireGeoObject(value.object),
    fields: fromWireFieldMap(value.fields),
    timestamp_ns:
      typeof value.timestamp_ns === "number"
        ? value.timestamp_ns
        : Number(value.timestamp_ns ?? 0),
    event_id: typeof value.event_id === "string" ? value.event_id : null,
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

export function fromWireHookDef(value: unknown): HookDef | null {
  if (value === null || value === undefined) {
    return null;
  }
  assertRecord(value, "hook definition");
  return {
    name: expectString(value.name, "hook name"),
    endpoint: expectString(value.endpoint, "hook endpoint"),
    def: fromWireGeofenceDef(value.def),
  };
}

function toWireFieldEntries(fields: FieldMap | FieldEntry[] | undefined): unknown[] {
  if (!fields) {
    return [];
  }
  const entries = Array.isArray(fields)
    ? fields
    : Object.entries(fields).map(([name, value]) => ({ name, value }));
  return entries.map((entry) => ({
    name: entry.name,
    value: toWireFieldValue(entry.value),
  }));
}

function toWireFieldValue(value: FieldValue): unknown {
  switch (value.type) {
    case "number":
      return { type: "number", value: value.value };
    case "text":
      return { type: "text", value: value.value };
    case "json":
      return { type: "json", value: JSON.stringify(value.value) };
  }
}

function fromWireFieldValue(value: unknown): FieldValue {
  assertRecord(value, "field value");
  if (value.type === "number") {
    return { type: "number", value: Number(value.value) };
  }
  if (value.type === "text") {
    return { type: "text", value: String(value.value) };
  }
  if (value.type === "json") {
    const raw = String(value.value);
    try {
      return { type: "json", value: JSON.parse(raw) as JsonValue };
    } catch {
      return { type: "json", value: raw };
    }
  }
  throw new Error("unknown field value shape");
}

function fromWireFieldMap(value: unknown): FieldMap {
  if (!isRecord(value) || !isRecord(value.values)) {
    return {};
  }
  return Object.fromEntries(
    Object.entries(value.values).map(([name, fieldValue]) => [
      name,
      fromWireFieldValue(fieldValue),
    ]),
  );
}

function toWireGeoObject(value: GeoObject): unknown {
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

function fromWireGeoObject(value: unknown): GeoObject {
  assertRecord(value, "geo object");
  if ("Point" in value) {
    const point = value.Point;
    assertRecord(point, "point");
    return {
      type: "point",
      lat: Number(point.lat),
      lon: Number(point.lon),
      z: typeof point.z === "number" || point.z === null ? point.z : undefined,
    };
  }
  if ("Bounds" in value) {
    return { type: "bounds", bounds: value.Bounds as BoundingBox };
  }
  if ("Hash" in value) {
    return { type: "hash", value: String(value.Hash) };
  }
  if ("GeoJson" in value) {
    return { type: "geojson", value: value.GeoJson as JsonValue };
  }
  if ("String" in value) {
    return { type: "string", value: String(value.String) };
  }
  throw new Error("unknown geo object shape");
}

function toWireArea(area: Area): unknown {
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

function toWireOutputFormat(value: OutputFormat | undefined): unknown {
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

function toWireWhereComparison(value: WhereComparison): unknown {
  switch (value.type) {
    case "range":
      return { Range: { min: value.min, max: value.max } };
    case "equalsText":
      return { EqualsText: value.value };
    case "regex":
      return { Regex: value.value };
  }
}

function toWireGeofenceQuery(value: GeofenceQuery): unknown {
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
      return { Within: { area: toWireArea(value.area), options: toWireSearchOptions(value.options) } };
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

function fromWireGeofenceDef(value: unknown): GeofenceDef {
  assertRecord(value, "geofence definition");
  const detect = value.detect;
  const commands = value.commands;
  if (!Array.isArray(detect) || !Array.isArray(commands)) {
    throw new Error("geofence detect and commands must be arrays");
  }
  return {
    collection: expectString(value.collection, "geofence collection"),
    query: fromWireGeofenceQuery(value.query),
    detect: detect.map(fromWireDetectType),
    commands: commands.map(fromWireMutationCommand),
  };
}

function fromWireGeofenceQuery(value: unknown): GeofenceQuery {
  assertRecord(value, "geofence query");
  if ("Nearby" in value) {
    const nearby = value.Nearby;
    assertRecord(nearby, "nearby query");
    return {
      type: "nearby",
      lat: Number(nearby.lat),
      lon: Number(nearby.lon),
      meters: Number(nearby.meters),
      options: fromWireSearchOptions(nearby.options),
    };
  }
  if ("Within" in value) {
    const within = value.Within;
    assertRecord(within, "within query");
    return {
      type: "within",
      area: fromWireArea(within.area),
      options: fromWireSearchOptions(within.options),
    };
  }
  if ("Intersects" in value) {
    const intersects = value.Intersects;
    assertRecord(intersects, "intersects query");
    return {
      type: "intersects",
      area: fromWireArea(intersects.area),
      options: fromWireSearchOptions(intersects.options),
    };
  }
  if ("Roam" in value) {
    const roam = value.Roam;
    assertRecord(roam, "roam query");
    return {
      type: "roam",
      targetCollection: expectString(roam.target_collection, "roam target collection"),
      targetPattern: expectString(roam.target_pattern, "roam target pattern"),
      meters: Number(roam.meters),
      options: fromWireSearchOptions(roam.options),
      nodwell: typeof roam.nodwell === "boolean" ? roam.nodwell : undefined,
    };
  }
  throw new Error("unknown geofence query shape");
}

function fromWireArea(value: unknown): Area {
  assertRecord(value, "area");
  if ("Circle" in value) {
    const circle = value.Circle;
    assertRecord(circle, "circle area");
    return {
      type: "circle",
      lat: Number(circle.lat),
      lon: Number(circle.lon),
      meters: Number(circle.meters),
    };
  }
  if ("Bounds" in value) {
    return { type: "bounds", bounds: value.Bounds as BoundingBox };
  }
  if ("Hash" in value) {
    return { type: "hash", value: String(value.Hash) };
  }
  if ("GeoJson" in value) {
    return { type: "geojson", value: value.GeoJson as JsonValue };
  }
  if ("Tile" in value) {
    const tile = value.Tile;
    assertRecord(tile, "tile area");
    return { type: "tile", x: Number(tile.x), y: Number(tile.y), z: Number(tile.z) };
  }
  if ("Quadkey" in value) {
    return { type: "quadkey", value: String(value.Quadkey) };
  }
  if ("Sector" in value) {
    const sector = value.Sector;
    assertRecord(sector, "sector area");
    return {
      type: "sector",
      lat: Number(sector.lat),
      lon: Number(sector.lon),
      meters: Number(sector.meters),
      bearing1: Number(sector.bearing1),
      bearing2: Number(sector.bearing2),
    };
  }
  if ("Reference" in value) {
    const reference = value.Reference;
    assertRecord(reference, "reference area");
    return {
      type: "reference",
      collection: expectString(reference.collection, "reference collection"),
      id: expectString(reference.id, "reference id"),
    };
  }
  throw new Error("unknown area shape");
}

function fromWireSearchOptions(value: unknown): SearchOptions | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  return {
    cursor: typeof value.cursor === "number" ? value.cursor : undefined,
    limit: typeof value.limit === "number" ? value.limit : undefined,
    nofields: typeof value.nofields === "boolean" ? value.nofields : undefined,
    matchPattern:
      typeof value.match_pattern === "string" ? value.match_pattern : undefined,
    sort: value.sort === "Desc" ? "desc" : value.sort === "Asc" ? "asc" : undefined,
    clip: typeof value.clip === "boolean" ? value.clip : undefined,
  };
}

function toWireSetCondition(value: SetObjectOptions["condition"] | undefined): unknown {
  switch (value) {
    case "nx":
      return "Nx";
    case "xx":
      return "Xx";
    case "always":
    case undefined:
      return "Always";
  }
}

function toWireMutationCommand(value: MutationCommand): string {
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

function fromWireMutationCommand(value: unknown): MutationCommand {
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

function toWireDetectType(value: DetectType): string {
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

function fromWireDetectType(value: unknown): DetectType {
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertRecord(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (!isRecord(value)) {
    throw new Error(`${label} must be an object`);
  }
}

function expectString(value: unknown, label: string): string {
  if (typeof value !== "string") {
    throw new Error(`${label} must be a string`);
  }
  return value;
}
