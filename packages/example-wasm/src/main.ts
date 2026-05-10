import * as L from "leaflet";
import {
  createLatLng,
  type CollectionInfo,
  type CollectionStats,
  type FieldMap,
  type GeofenceEvent,
  type GeoObject,
  type LatLngObject,
  type LatLngWasmClient,
  type MutationEvent,
  type SearchOptions,
  type SearchResults,
  type ServerInfo,
} from "@latlng/wasm";
import "leaflet/dist/leaflet.css";
import "./style.css";

const COLLECTION = "fleet";
const HOOK_NAME = "alexanderplatz-arrivals";
const HOOK_CENTER = { lat: 52.5219, lon: 13.4132 };
const HOOK_RADIUS_METERS = 350;

const BERLIN_VIEW = {
  minLat: 52.47,
  minLon: 13.31,
  maxLat: 52.56,
  maxLon: 13.49,
};
const BERLIN_CENTER = L.latLng(52.52, 13.405);

type VehicleStatus = "active" | "idle" | "maintenance";

interface FleetPoint {
  id: string;
  name: string;
  kind: string;
  status: VehicleStatus;
  speed: number;
  route: string;
  lat: number;
  lon: number;
}

interface EventLogEntry {
  id: number;
  time: string;
  type: string;
  label: string;
  detail: unknown;
}

type QueryMode = "nearby" | "within" | "intersects" | "scan" | "search";

const seedFleet: FleetPoint[] = [
  {
    id: "courier-mitte",
    name: "Courier Mitte",
    kind: "bike",
    status: "active",
    speed: 18,
    route: "museum-island",
    lat: 52.5163,
    lon: 13.3777,
  },
  {
    id: "van-prenzlauer",
    name: "Van Prenzlauer",
    kind: "van",
    status: "active",
    speed: 33,
    route: "north-ring",
    lat: 52.5328,
    lon: 13.4147,
  },
  {
    id: "drone-tiergarten",
    name: "Drone Tiergarten",
    kind: "drone",
    status: "idle",
    speed: 0,
    route: "park-watch",
    lat: 52.5145,
    lon: 13.3501,
  },
  {
    id: "cargo-kreuzberg",
    name: "Cargo Kreuzberg",
    kind: "cargo-bike",
    status: "maintenance",
    speed: 4,
    route: "canal",
    lat: 52.4996,
    lon: 13.4314,
  },
  {
    id: "shuttle-friedrichshain",
    name: "Shuttle Friedrichshain",
    kind: "shuttle",
    status: "active",
    speed: 27,
    route: "east-hub",
    lat: 52.5158,
    lon: 13.4548,
  },
];

const queryLabels: Record<QueryMode, string> = {
  nearby: "Nearby",
  within: "Within",
  intersects: "Intersects",
  scan: "Scan",
  search: "Search",
};

let db: LatLngWasmClient | null = null;
let queryMode: QueryMode = "nearby";
let activeId = seedFleet[0]?.id ?? "";
let events: EventLogEntry[] = [];
let eventCounter = 1;
let customCounter = 1;
let latestObjects: LatLngObject[] = [];
let suppressRefresh = false;
let map: L.Map;
let markerLayer: L.LayerGroup;
let queryLayer: L.LayerGroup;
let hookLayer: L.LayerGroup;
let highlightedResultIds = new Set<string>();
const markers = new Map<string, L.Marker>();
const suppressedMutations: Array<Pick<MutationEvent, "type" | "hook">> = [];

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("missing #app root");
}

app.innerHTML = `
  <div class="shell">
    <header class="topbar">
      <div>
        <p class="eyebrow">@latlng/wasm</p>
        <h1>latlng wasm workbench</h1>
      </div>
      <div class="status-wrap">
        <span id="runtimeStatus" class="status-pill">Starting</span>
        <button id="resetButton" type="button" class="button secondary">Reset data</button>
      </div>
    </header>

    <section class="metrics" aria-label="Engine metrics">
      <div><span>Version</span><strong id="metricVersion">-</strong></div>
      <div><span>Collections</span><strong id="metricCollections">-</strong></div>
      <div><span>Objects</span><strong id="metricObjects">-</strong></div>
      <div><span>Points</span><strong id="metricPoints">-</strong></div>
      <div><span>Heap</span><strong id="metricHeap">-</strong></div>
    </section>

    <main class="workspace">
      <section class="panel controls" aria-label="Controls">
        <div class="panel-head">
          <h2>Operations</h2>
        </div>

        <div class="control-group">
          <label for="activeObject">Object</label>
          <select id="activeObject"></select>
        </div>

        <div class="button-grid">
          <button id="addObjectButton" type="button" class="button">Add object</button>
          <button id="deleteObjectButton" type="button" class="button secondary">Delete object</button>
          <button id="expireButton" type="button" class="button secondary">Expire 120s</button>
          <button id="ttlButton" type="button" class="button secondary">Read TTL</button>
          <button id="persistButton" type="button" class="button secondary">Persist</button>
          <button id="infoButton" type="button" class="button secondary">Refresh info</button>
        </div>

        <div class="divider"></div>

        <div class="tabs" role="tablist" aria-label="Query mode">
          <button type="button" class="tab active" data-query="nearby">Nearby</button>
          <button type="button" class="tab" data-query="within">Within</button>
          <button type="button" class="tab" data-query="intersects">Intersects</button>
          <button type="button" class="tab" data-query="scan">Scan</button>
          <button type="button" class="tab" data-query="search">Search</button>
        </div>

        <div class="form-grid">
          <label>
            <span>Radius m</span>
            <input id="radiusInput" type="number" min="50" step="50" value="1200" />
          </label>
          <label>
            <span>Limit</span>
            <input id="limitInput" type="number" min="1" max="100" value="20" />
          </label>
          <label>
            <span>Sort</span>
            <select id="sortInput">
              <option value="asc">Asc</option>
              <option value="desc">Desc</option>
            </select>
          </label>
          <label>
            <span>Pattern</span>
            <input id="patternInput" type="text" value="*" />
          </label>
        </div>

        <label class="check-row">
          <input id="activeOnlyInput" type="checkbox" />
          <span>Active only</span>
        </label>

        <button id="runQueryButton" type="button" class="button primary">Run query</button>

        <div class="divider"></div>

        <div class="button-grid">
          <button id="setHookButton" type="button" class="button">Set hook</button>
          <button id="moveInButton" type="button" class="button secondary">Move in</button>
          <button id="moveOutButton" type="button" class="button secondary">Move out</button>
          <button id="deleteHookButton" type="button" class="button secondary">Delete hook</button>
        </div>
      </section>

      <section class="panel map-panel" aria-label="Fleet view">
        <div class="panel-head">
          <div>
            <h2>Fleet</h2>
            <p id="collectionSummary">-</p>
          </div>
          <span id="boundsSummary" class="mini-pill">No bounds</span>
        </div>
        <div id="map" class="map-surface"></div>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>ID</th>
                <th>Object</th>
                <th>Status</th>
                <th>Position</th>
              </tr>
            </thead>
            <tbody id="objectRows"></tbody>
          </table>
        </div>
      </section>

      <aside class="panel output" aria-label="Output">
        <div class="panel-head event-head">
          <h2>Hook events</h2>
          <span id="eventCount" class="mini-pill">0</span>
        </div>
        <ol id="eventLog" class="event-log"></ol>

        <div class="panel-head">
          <h2 id="resultTitle">Result</h2>
        </div>
        <pre id="resultJson" class="json-block">{}</pre>
      </aside>
    </main>
  </div>
`;

const ui = {
  runtimeStatus: byId<HTMLSpanElement>("runtimeStatus"),
  resetButton: byId<HTMLButtonElement>("resetButton"),
  metricVersion: byId<HTMLElement>("metricVersion"),
  metricCollections: byId<HTMLElement>("metricCollections"),
  metricObjects: byId<HTMLElement>("metricObjects"),
  metricPoints: byId<HTMLElement>("metricPoints"),
  metricHeap: byId<HTMLElement>("metricHeap"),
  activeObject: byId<HTMLSelectElement>("activeObject"),
  addObjectButton: byId<HTMLButtonElement>("addObjectButton"),
  deleteObjectButton: byId<HTMLButtonElement>("deleteObjectButton"),
  expireButton: byId<HTMLButtonElement>("expireButton"),
  ttlButton: byId<HTMLButtonElement>("ttlButton"),
  persistButton: byId<HTMLButtonElement>("persistButton"),
  infoButton: byId<HTMLButtonElement>("infoButton"),
  radiusInput: byId<HTMLInputElement>("radiusInput"),
  limitInput: byId<HTMLInputElement>("limitInput"),
  sortInput: byId<HTMLSelectElement>("sortInput"),
  patternInput: byId<HTMLInputElement>("patternInput"),
  activeOnlyInput: byId<HTMLInputElement>("activeOnlyInput"),
  runQueryButton: byId<HTMLButtonElement>("runQueryButton"),
  setHookButton: byId<HTMLButtonElement>("setHookButton"),
  moveInButton: byId<HTMLButtonElement>("moveInButton"),
  moveOutButton: byId<HTMLButtonElement>("moveOutButton"),
  deleteHookButton: byId<HTMLButtonElement>("deleteHookButton"),
  collectionSummary: byId<HTMLElement>("collectionSummary"),
  boundsSummary: byId<HTMLElement>("boundsSummary"),
  map: byId<HTMLDivElement>("map"),
  objectRows: byId<HTMLTableSectionElement>("objectRows"),
  resultTitle: byId<HTMLElement>("resultTitle"),
  resultJson: byId<HTMLElement>("resultJson"),
  eventCount: byId<HTMLElement>("eventCount"),
  eventLog: byId<HTMLOListElement>("eventLog"),
};

initMap();
wireUi();
void boot();

function byId<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`missing #${id}`);
  }
  return element as T;
}

function initMap(): void {
  map = L.map(ui.map, {
    preferCanvas: true,
    zoomControl: true,
  }).setView(BERLIN_CENTER, 12);

  L.tileLayer("https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png", {
    attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>',
    maxZoom: 19,
  }).addTo(map);

  markerLayer = L.layerGroup().addTo(map);
  queryLayer = L.layerGroup().addTo(map);
  hookLayer = L.layerGroup().addTo(map);
  L.control.scale({ imperial: false, metric: true }).addTo(map);

  requestAnimationFrame(() => {
    map.invalidateSize();
  });
}

function wireUi(): void {
  ui.resetButton.addEventListener("click", () => runAction("Reset data", resetData));
  ui.infoButton.addEventListener("click", () => runAction("Refresh info", refresh));
  ui.addObjectButton.addEventListener("click", () => runAction("Add object", addObject));
  ui.deleteObjectButton.addEventListener("click", () => runAction("Delete object", deleteActiveObject));
  ui.expireButton.addEventListener("click", () => runAction("Expire object", expireActiveObject));
  ui.ttlButton.addEventListener("click", () => runAction("Read TTL", readActiveTtl));
  ui.persistButton.addEventListener("click", () => runAction("Persist object", persistActiveObject));
  ui.runQueryButton.addEventListener("click", () => runAction("Run query", runSelectedQuery));
  ui.setHookButton.addEventListener("click", () => runAction("Set hook", setHook));
  ui.moveInButton.addEventListener("click", () => runAction("Move in", () => moveActiveObject("in")));
  ui.moveOutButton.addEventListener("click", () => runAction("Move out", () => moveActiveObject("out")));
  ui.deleteHookButton.addEventListener("click", () => runAction("Delete hook", deleteHook));
  ui.activeObject.addEventListener("change", () => {
    selectObject(ui.activeObject.value, true, true);
  });

  for (const tab of document.querySelectorAll<HTMLButtonElement>("[data-query]")) {
    tab.addEventListener("click", () => {
      queryMode = readQueryMode(tab.dataset.query);
      for (const current of document.querySelectorAll<HTMLButtonElement>("[data-query]")) {
        current.classList.toggle("active", current === tab);
      }
      ui.resultTitle.textContent = `${queryLabels[queryMode]} result`;
    });
  }
}

async function boot(): Promise<void> {
  setStatus("Starting", "pending");
  try {
    db = await createLatLng();
    db.addEventListener("mutation", (event) => {
      if (shouldSuppressMutation(event.detail)) {
        return;
      }
      pushEvent("mutation", mutationLabel(event.detail), event.detail);
      if (!suppressRefresh) {
        void refresh().catch((error: unknown) => {
          showResult("Refresh error", errorToJson(error));
        });
      }
    });
    db.addEventListener("geofence", (event) => {
      pushEvent("hook", geofenceLabel(event.detail), event.detail);
      if (!suppressRefresh) {
        void refresh().catch((error: unknown) => {
          showResult("Refresh error", errorToJson(error));
        });
      }
    });
    setStatus("Ready", "ready");
    await resetData();
  } catch (error) {
    setStatus("Failed", "error");
    showResult("Startup error", errorToJson(error));
  }
}

async function runAction(label: string, action: () => Promise<void>): Promise<void> {
  if (!db) {
    showResult(label, { error: "latlng wasm client is not ready" });
    return;
  }
  setButtonsDisabled(true);
  setStatus(label, "pending");
  try {
    await action();
    setStatus("Ready", "ready");
  } catch (error) {
    setStatus("Error", "error");
    showResult(`${label} error`, errorToJson(error));
  } finally {
    setButtonsDisabled(false);
  }
}

async function resetData(): Promise<void> {
  const client = requireClient();
  suppressRefresh = true;
  try {
    await safeDeleteHook();
    await client.dropCollection(COLLECTION).catch(() => false);
    await client.createCollection(COLLECTION);

    for (const point of seedFleet) {
      await writeFleetPoint(point);
    }

    await client.setObject(
      COLLECTION,
      "note-hub",
      { type: "string", value: "berlin central dispatch priority active" },
      {
        fields: {
          name: { type: "text", value: "Central dispatch note" },
          status: { type: "text", value: "active" },
          kind: { type: "text", value: "note" },
          priority: { type: "number", value: 1 },
        },
      },
    );
  } finally {
    suppressRefresh = false;
  }

  activeId = seedFleet[0]?.id ?? "";
  events = [];
  eventCounter = 1;
  customCounter = 1;
  await refresh();
  await runQuery("nearby");
}

async function refresh(): Promise<void> {
  const client = requireClient();
  const [info, collectionInfo, stats, objects] = await Promise.all([
    client.serverInfo(),
    client.collectionInfo(COLLECTION),
    client.stats([COLLECTION]),
    readObjects(),
  ]);

  latestObjects = objects;
  renderMetrics(info);
  renderCollection(collectionInfo, stats[0] ?? null);
  renderObjectSelect();
  renderObjects();
  renderEvents();
}

async function readObjects(): Promise<LatLngObject[]> {
  const client = requireClient();
  const result = await client
    .scan(COLLECTION, { limit: 100, output: "objects" })
    .catch((error: unknown) => {
      if (error instanceof Error && error.message.includes("collection not found")) {
        return null;
      }
      throw error;
    });
  if (!result) {
    return [];
  }
  const objects = await Promise.all(
    result.results.map((item) => client.getObject(COLLECTION, item.id)),
  );
  return objects.filter((object): object is LatLngObject => object !== null);
}

async function runSelectedQuery(): Promise<void> {
  await runQuery(queryMode);
}

async function runQuery(mode: QueryMode): Promise<void> {
  const client = requireClient();
  const options = buildSearchOptions();
  let result: SearchResults;

  if (mode === "nearby") {
    result = await client.nearby(COLLECTION, {
      lat: 52.5208,
      lon: 13.4095,
      meters: readNumber(ui.radiusInput, 1200),
      options,
    });
  } else if (mode === "within") {
    result = await client.within(COLLECTION, {
      area: {
        type: "circle",
        lat: 52.5155,
        lon: 13.3777,
        meters: readNumber(ui.radiusInput, 1200),
      },
      options,
    });
  } else if (mode === "intersects") {
    result = await client.intersects(COLLECTION, {
      area: {
        type: "bounds",
        bounds: {
          min_lat: 52.49,
          min_lon: 13.36,
          max_lat: 52.535,
          max_lon: 13.44,
        },
      },
      options,
    });
  } else if (mode === "search") {
    result = await client.search(COLLECTION, {
      ...options,
      matchPattern: ui.patternInput.value.trim() || "note-*",
    });
  } else {
    result = await client.scan(COLLECTION, options);
  }

  showResult(`${queryLabels[mode]} result`, result);
  renderQueryOverlay(mode, result);
}

async function addObject(): Promise<void> {
  const index = customCounter++;
  const point: FleetPoint = {
    id: `dispatch-${String(index).padStart(2, "0")}`,
    name: `Dispatch ${index}`,
    kind: "scooter",
    status: "active",
    speed: 12 + index,
    route: "ad-hoc",
    lat: 52.487 + (index % 5) * 0.011,
    lon: 13.337 + (index % 7) * 0.017,
  };

  await writeFleetPoint(point);
  activeId = point.id;
  await refresh();
  showResult("Added object", await requireClient().getObject(COLLECTION, point.id));
}

async function deleteActiveObject(): Promise<void> {
  const id = readActiveId();
  const changed = await requireClient().deleteObject(COLLECTION, id);
  activeId = latestObjects.find((object) => object.id !== id)?.id ?? seedFleet[0]?.id ?? "";
  await refresh();
  showResult("Delete object", { id, changed });
}

async function expireActiveObject(): Promise<void> {
  const id = readActiveId();
  const changed = await requireClient().expire(COLLECTION, id, 120);
  const ttl = await requireClient().ttl(COLLECTION, id);
  await refresh();
  showResult("Expire object", { id, changed, ttl_seconds: ttl });
}

async function readActiveTtl(): Promise<void> {
  const id = readActiveId();
  const ttl = await requireClient().ttl(COLLECTION, id);
  showResult("Object TTL", { id, ttl_seconds: ttl });
}

async function persistActiveObject(): Promise<void> {
  const id = readActiveId();
  const changed = await requireClient().persist(COLLECTION, id);
  const ttl = await requireClient().ttl(COLLECTION, id);
  await refresh();
  showResult("Persist object", { id, changed, ttl_seconds: ttl });
}

async function setHook(): Promise<void> {
  const hook = await ensureHook();
  showResult("Set hook", hook);
}

async function ensureHook(options: { suppressMutationLog?: boolean } = {}): Promise<unknown> {
  const client = requireClient();
  const existing = await client.getHook(HOOK_NAME);
  if (existing) {
    renderHookOverlay();
    return existing;
  }

  const suppressedMutation: Pick<MutationEvent, "type" | "hook"> = {
    type: "hook:set",
    hook: HOOK_NAME,
  };
  if (options.suppressMutationLog) {
    suppressedMutations.push(suppressedMutation);
  }

  try {
    await client.setHook(HOOK_NAME, {
      collection: COLLECTION,
      query: {
        type: "nearby",
        lat: HOOK_CENTER.lat,
        lon: HOOK_CENTER.lon,
        meters: HOOK_RADIUS_METERS,
      },
      detect: ["enter", "exit"],
      commands: ["set", "del"],
    });
  } catch (error) {
    discardSuppressedMutation(suppressedMutation);
    throw error;
  }
  renderHookOverlay();
  return client.getHook(HOOK_NAME);
}

async function moveActiveObject(direction: "in" | "out"): Promise<void> {
  await ensureHook({ suppressMutationLog: true });
  const object = getActiveFleetPoint();
  const inside = { lat: 52.522, lon: 13.4129, label: "Alexanderplatz" };
  const outside = { lat: 52.5163, lon: 13.3777, label: "Brandenburg Gate" };
  const isInside = isInsideHook(object);
  const target = direction === "in" ? inside : outside;

  if ((direction === "in" && isInside) || (direction === "out" && !isInside)) {
    renderHookOverlay();
    showResult("Move object", {
      id: object.id,
      skipped: true,
      reason: `already ${direction === "in" ? "inside" : "outside"} hook`,
      lat: object.lat,
      lon: object.lon,
    });
    return;
  }

  const point: FleetPoint = { ...object, lat: target.lat, lon: target.lon, status: "active" };

  await writeFleetPoint(point);
  await refresh();
  showResult("Move object", {
    id: point.id,
    target: target.label,
    lat: point.lat,
    lon: point.lon,
  });
}

async function deleteHook(): Promise<void> {
  const changed = await safeDeleteHook();
  hookLayer.clearLayers();
  showResult("Delete hook", { hook: HOOK_NAME, changed });
}

async function safeDeleteHook(): Promise<boolean> {
  const client = requireClient();
  try {
    return await client.deleteHook(HOOK_NAME);
  } catch {
    return false;
  }
}

async function writeFleetPoint(point: FleetPoint): Promise<void> {
  await requireClient().setPoint(
    COLLECTION,
    point.id,
    { lat: point.lat, lon: point.lon },
    { fields: fieldsForPoint(point) },
  );
}

function fieldsForPoint(point: FleetPoint): FieldMap {
  return {
    name: { type: "text", value: point.name },
    kind: { type: "text", value: point.kind },
    status: { type: "text", value: point.status },
    speed: { type: "number", value: point.speed },
    route: { type: "text", value: point.route },
    payload: {
      type: "json",
      value: {
        route: point.route,
        active: point.status === "active",
      },
    },
  };
}

function buildSearchOptions(): SearchOptions {
  const options: SearchOptions = {
    limit: readNumber(ui.limitInput, 20),
    sort: ui.sortInput.value === "desc" ? "desc" : "asc",
    output: "objects",
    matchPattern: ui.patternInput.value.trim() || "*",
  };

  if (ui.activeOnlyInput.checked) {
    options.whereFilters = [
      {
        field: "status",
        comparison: { type: "equalsText", value: "active" },
      },
    ];
  }

  return options;
}

function renderMetrics(info: ServerInfo): void {
  ui.metricVersion.textContent = info.version || "dev";
  ui.metricCollections.textContent = String(info.num_collections);
  ui.metricObjects.textContent = String(info.num_objects);
  ui.metricPoints.textContent = String(info.num_points);
  ui.metricHeap.textContent = formatBytes(info.heap_bytes);
}

function renderCollection(info: CollectionInfo | null, stats: CollectionStats | null): void {
  const objectCount = stats?.object_count ?? info?.stats.object_count ?? 0;
  const pointCount = stats?.point_count ?? info?.stats.point_count ?? 0;
  const stringCount = stats?.string_count ?? info?.stats.string_count ?? 0;
  const expiresCount = stats?.expires_count ?? info?.stats.expires_count ?? 0;

  ui.collectionSummary.textContent = `${objectCount} objects, ${pointCount} points, ${stringCount} strings, ${expiresCount} TTL`;

  if (!info?.bounds) {
    ui.boundsSummary.textContent = "No bounds";
    return;
  }

  ui.boundsSummary.textContent = `${info.bounds.min_lat.toFixed(3)}, ${info.bounds.min_lon.toFixed(3)} to ${info.bounds.max_lat.toFixed(3)}, ${info.bounds.max_lon.toFixed(3)}`;
}

function renderObjectSelect(): void {
  const previous = activeId;
  ui.activeObject.replaceChildren();

  for (const object of latestObjects) {
    const option = document.createElement("option");
    option.value = object.id;
    option.textContent = object.id;
    ui.activeObject.append(option);
  }

  const fallback = latestObjects[0]?.id ?? "";
  activeId = latestObjects.some((object) => object.id === previous) ? previous : fallback;
  ui.activeObject.value = activeId;
}

function renderObjects(): void {
  markerLayer.clearLayers();
  markers.clear();
  ui.objectRows.replaceChildren();

  for (const object of latestObjects) {
    const row = document.createElement("tr");
    row.classList.toggle("selected", object.id === activeId);
    row.append(
      tableCell(object.id),
      tableCell(objectLabel(object.geo, object.fields)),
      tableCell(fieldText(object.fields, "status")),
      tableCell(positionLabel(object.geo)),
    );
    row.addEventListener("click", () => {
      selectObject(object.id, true, true);
    });
    ui.objectRows.append(row);

    if (object.geo.type === "point") {
      renderMarker(object);
    }
  }

  zoomToCollection();
}

function renderMarker(object: LatLngObject): void {
  const geo = object.geo;
  if (geo.type !== "point") {
    return;
  }

  const marker = L.marker([geo.lat, geo.lon], {
    icon: L.divIcon({
      className: markerClassName(object),
      html: `<span>${initials(object.id)}</span>`,
      iconAnchor: [17, 17],
      iconSize: [34, 34],
      popupAnchor: [0, -18],
    }),
    title: object.id,
  });
  marker.bindPopup(popupHtml(object));
  marker.on("click", () => {
    selectObject(object.id, false, true);
  });
  marker.addTo(markerLayer);
  markers.set(object.id, marker);
}

function selectObject(id: string, panToMarker: boolean, openPopup: boolean): void {
  if (!id) {
    return;
  }

  activeId = id;
  ui.activeObject.value = id;
  renderObjects();

  const marker = markers.get(id);
  if (!marker) {
    return;
  }

  if (panToMarker) {
    map.panTo(marker.getLatLng(), { animate: true });
  }
  if (openPopup) {
    marker.openPopup();
  }
}

function renderQueryOverlay(mode: QueryMode, result: SearchResults): void {
  queryLayer.clearLayers();
  highlightedResultIds = new Set(result.results.map((item) => item.id));
  renderObjects();

  const bounds = L.latLngBounds([]);
  for (const item of result.results) {
    if (item.object?.type === "point") {
      bounds.extend([item.object.lat, item.object.lon]);
    }
  }

  if (mode === "nearby") {
    const center = L.latLng(52.5208, 13.4095);
    const radius = readNumber(ui.radiusInput, 1200);
    L.circle(center, {
      radius,
      color: "#164f5f",
      fillColor: "#3aa6b9",
      fillOpacity: 0.12,
      weight: 2,
    }).addTo(queryLayer);
    bounds.extend(center);
  } else if (mode === "within") {
    const center = L.latLng(52.5155, 13.3777);
    const radius = readNumber(ui.radiusInput, 1200);
    L.circle(center, {
      radius,
      color: "#3d8a56",
      fillColor: "#54b96f",
      fillOpacity: 0.12,
      weight: 2,
    }).addTo(queryLayer);
    bounds.extend(center);
  } else if (mode === "intersects") {
    const areaBounds = L.latLngBounds([52.49, 13.36], [52.535, 13.44]);
    L.rectangle(areaBounds, {
      color: "#946d16",
      fillColor: "#e0a93a",
      fillOpacity: 0.1,
      weight: 2,
    }).addTo(queryLayer);
    bounds.extend(areaBounds);
  }

  if (bounds.isValid()) {
    map.fitBounds(bounds.pad(0.25), { maxZoom: 14 });
  }
}

function renderHookOverlay(): void {
  hookLayer.clearLayers();
  L.circle([HOOK_CENTER.lat, HOOK_CENTER.lon], {
    radius: HOOK_RADIUS_METERS,
    color: "#7b3f98",
    fillColor: "#b06cc8",
    fillOpacity: 0.1,
    weight: 2,
    dashArray: "6 5",
  })
    .bindTooltip(HOOK_NAME, { direction: "top", opacity: 0.9 })
    .addTo(hookLayer);
}

function zoomToCollection(): void {
  const bounds = L.latLngBounds([]);
  for (const object of latestObjects) {
    if (object.geo.type === "point") {
      bounds.extend([object.geo.lat, object.geo.lon]);
    }
  }

  if (bounds.isValid()) {
    map.fitBounds(bounds.pad(0.2), { maxZoom: 13, animate: false });
  } else {
    map.setView(BERLIN_CENTER, 12);
  }

  requestAnimationFrame(() => {
    map.invalidateSize();
  });
}

function markerClassName(object: LatLngObject): string {
  const status = fieldText(object.fields, "status") || "unknown";
  return [
    "fleet-marker",
    `status-${status}`,
    object.id === activeId ? "selected" : "",
    highlightedResultIds.has(object.id) ? "matched" : "",
  ]
    .filter(Boolean)
    .join(" ");
}

function popupHtml(object: LatLngObject): string {
  return [
    `<strong>${escapeHtml(objectLabel(object.geo, object.fields))}</strong>`,
    `<span>${escapeHtml(object.id)}</span>`,
    `<span>${escapeHtml(fieldText(object.fields, "status") || "unknown")}</span>`,
    `<span>${escapeHtml(positionLabel(object.geo))}</span>`,
  ].join("");
}

function initials(id: string): string {
  return id
    .split(/[-_\s]+/)
    .map((part) => part[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (match) => {
    switch (match) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      case "'":
        return "&#39;";
      default:
        return match;
    }
  });
}

function renderEvents(): void {
  ui.eventCount.textContent = String(events.length);
  ui.eventLog.replaceChildren();

  for (const event of events.slice(0, 12)) {
    const item = document.createElement("li");
    const head = document.createElement("div");
    const type = document.createElement("span");
    const time = document.createElement("time");
    const label = document.createElement("p");

    head.className = "event-row";
    type.className = `event-type ${event.type}`;
    type.textContent = event.type;
    time.textContent = event.time;
    label.textContent = event.label;
    head.append(type, time);
    item.append(head, label);
    item.addEventListener("click", () => showResult("Event detail", event.detail));
    ui.eventLog.append(item);
  }
}

function pushEvent(type: string, label: string, detail: unknown): void {
  events = [
    {
      id: eventCounter++,
      time: new Date().toLocaleTimeString(),
      type,
      label,
      detail,
    },
    ...events,
  ].slice(0, 30);
  renderEvents();
}

function showResult(title: string, value: unknown): void {
  ui.resultTitle.textContent = title;
  ui.resultJson.textContent = JSON.stringify(value, null, 2);
}

function setStatus(label: string, state: "ready" | "pending" | "error"): void {
  ui.runtimeStatus.textContent = label;
  ui.runtimeStatus.dataset.state = state;
}

function setButtonsDisabled(disabled: boolean): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>("button")) {
    button.disabled = disabled;
  }
}

function readQueryMode(value: string | undefined): QueryMode {
  if (
    value === "nearby" ||
    value === "within" ||
    value === "intersects" ||
    value === "scan" ||
    value === "search"
  ) {
    return value;
  }
  return "nearby";
}

function readActiveId(): string {
  const id = ui.activeObject.value || activeId;
  if (!id) {
    throw new Error("no active object selected");
  }
  return id;
}

function readNumber(input: HTMLInputElement, fallback: number): number {
  const value = Number(input.value);
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

function getActiveFleetPoint(): FleetPoint {
  const object = latestObjects.find((entry) => entry.id === readActiveId());
  const geo = object?.geo;
  return {
    id: object?.id ?? "courier-mitte",
    name: fieldText(object?.fields, "name") || "Courier Mitte",
    kind: fieldText(object?.fields, "kind") || "bike",
    status: "active",
    speed: Number(fieldValue(object?.fields, "speed") ?? 18),
    route: fieldText(object?.fields, "route") || "manual",
    lat: geo?.type === "point" ? geo.lat : 52.5163,
    lon: geo?.type === "point" ? geo.lon : 13.3777,
  };
}

function isInsideHook(point: Pick<FleetPoint, "lat" | "lon">): boolean {
  return distanceMeters(point.lat, point.lon, HOOK_CENTER.lat, HOOK_CENTER.lon) <= HOOK_RADIUS_METERS;
}

function distanceMeters(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const earthRadiusMeters = 6_371_000;
  const toRadians = Math.PI / 180;
  const phi1 = lat1 * toRadians;
  const phi2 = lat2 * toRadians;
  const deltaPhi = (lat2 - lat1) * toRadians;
  const deltaLambda = (lon2 - lon1) * toRadians;
  const a =
    Math.sin(deltaPhi / 2) ** 2 +
    Math.cos(phi1) * Math.cos(phi2) * Math.sin(deltaLambda / 2) ** 2;
  return earthRadiusMeters * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
}

function tableCell(value: string): HTMLTableCellElement {
  const cell = document.createElement("td");
  cell.textContent = value;
  return cell;
}

function objectLabel(object: GeoObject, fields: FieldMap): string {
  const name = fieldText(fields, "name");
  if (name) {
    return name;
  }
  if (object.type === "string") {
    return object.value;
  }
  return object.type;
}

function positionLabel(object: GeoObject): string {
  if (object.type === "point") {
    return `${object.lat.toFixed(4)}, ${object.lon.toFixed(4)}`;
  }
  return object.type;
}

function fieldText(fields: FieldMap | undefined, key: string): string {
  const value = fieldValue(fields, key);
  return value === null || value === undefined ? "" : String(value);
}

function fieldValue(fields: FieldMap | undefined, key: string): unknown {
  const field = fields?.[key];
  if (!field) {
    return null;
  }
  return field.value;
}

function plotPosition(lat: number, lon: number): { x: number; y: number } {
  const x = ((lon - BERLIN_VIEW.minLon) / (BERLIN_VIEW.maxLon - BERLIN_VIEW.minLon)) * 100;
  const y = 100 - ((lat - BERLIN_VIEW.minLat) / (BERLIN_VIEW.maxLat - BERLIN_VIEW.minLat)) * 100;
  return {
    x: clamp(x, 3, 97),
    y: clamp(y, 3, 97),
  };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function mutationLabel(event: MutationEvent): string {
  if (event.id) {
    return `${event.type} ${event.collection ?? COLLECTION}/${event.id}`;
  }
  if (event.hook) {
    return `${event.type} ${event.hook}`;
  }
  return `${event.type} ${event.collection ?? ""}`.trim();
}

function shouldSuppressMutation(event: MutationEvent): boolean {
  const index = suppressedMutations.findIndex(
    (mutation) => mutation.type === event.type && mutation.hook === event.hook,
  );
  if (index === -1) {
    return false;
  }
  suppressedMutations.splice(index, 1);
  return true;
}

function discardSuppressedMutation(mutation: Pick<MutationEvent, "type" | "hook">): void {
  const index = suppressedMutations.findIndex(
    (entry) => entry.type === mutation.type && entry.hook === mutation.hook,
  );
  if (index !== -1) {
    suppressedMutations.splice(index, 1);
  }
}

function geofenceLabel(event: GeofenceEvent): string {
  return `${event.detect} ${event.collection}/${event.id} via ${event.hook ?? HOOK_NAME}`;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const kib = bytes / 1024;
  if (kib < 1024) {
    return `${kib.toFixed(1)} KiB`;
  }
  return `${(kib / 1024).toFixed(1)} MiB`;
}

function requireClient(): LatLngWasmClient {
  if (!db) {
    throw new Error("latlng wasm client is not ready");
  }
  return db;
}

function errorToJson(error: unknown): { message: string } {
  return { message: error instanceof Error ? error.message : String(error) };
}
