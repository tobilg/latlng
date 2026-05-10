import { describe, expect, it } from "vitest";
import { createLatLng, LatLngWasmClient } from "../src/index.js";
import type { WorkerOutboundMessage, WorkerRequest } from "../src/messages.js";

class FakeWorker extends EventTarget {
  public requests: WorkerRequest[] = [];
  public terminated = false;

  public postMessage(message: WorkerRequest): void {
    this.requests.push(message);
  }

  public terminate(): void {
    this.terminated = true;
  }

  public reply(id: number, result: unknown): void {
    this.dispatchEvent(
      new MessageEvent("message", { data: { id, ok: true, result } satisfies WorkerOutboundMessage }),
    );
  }

  public fail(id: number, error: string): void {
    this.dispatchEvent(
      new MessageEvent("message", { data: { id, ok: false, error } satisfies WorkerOutboundMessage }),
    );
  }

  public emit(message: WorkerOutboundMessage): void {
    this.dispatchEvent(new MessageEvent("message", { data: message }));
  }
}

describe("LatLngWasmClient", () => {
  it("routes requests through a worker and correlates responses", async () => {
    const worker = new FakeWorker();
    const client = new LatLngWasmClient(worker as unknown as Worker);

    const collections = client.collections();
    const created = client.createCollection("fleet");

    expect(worker.requests.map((request) => request.method)).toEqual([
      "init",
      "collections",
      "createCollection",
    ]);

    worker.reply(worker.requests[2]!.id, true);
    worker.reply(worker.requests[1]!.id, ["fleet"]);

    await expect(created).resolves.toBe(true);
    await expect(collections).resolves.toEqual(["fleet"]);
  });

  it("emits typed mutation and geofence events from worker messages", async () => {
    const worker = new FakeWorker();
    const client = new LatLngWasmClient(worker as unknown as Worker);
    const mutations: unknown[] = [];
    const geofences: unknown[] = [];
    const enters: unknown[] = [];

    client.addEventListener("mutation", (event) => mutations.push(event.detail));
    client.addEventListener("geofence", (event) => geofences.push(event.detail));
    client.addEventListener("geofence:enter", (event) => enters.push(event.detail));

    worker.emit({
      type: "event",
      eventType: "mutation",
      event: { type: "object:set", collection: "fleet", id: "truck-1", changed: true },
    });
    worker.emit({
      type: "event",
      eventType: "geofence",
      event: {
        command: "set",
        detect: "enter",
        collection: "fleet",
        id: "truck-1",
        object: { type: "point", lat: 52.5, lon: 13.4 },
        fields: {},
        timestamp_ns: 1,
        event_id: "evt-1",
        hook: "berlin",
      },
    });

    expect(mutations).toHaveLength(1);
    expect(geofences).toHaveLength(1);
    expect(enters).toHaveLength(1);
  });

  it("creates a client with a worker factory and performs the ready probe", async () => {
    const worker = new FakeWorker();
    const created = createLatLng({
      workerFactory: () => worker as unknown as Worker,
    });

    expect(worker.requests[0]?.method).toBe("init");
    worker.reply(worker.requests[0]!.id, {
      version: "0.1.0",
      api_version: "v1",
      protocol_version: "capnp-v1",
      storage_format_version: "storage-v1",
      num_collections: 0,
      num_objects: 0,
      num_points: 0,
      heap_bytes: 0,
      read_only: false,
      leader: true,
      last_sequence: 0,
    });

    const client = await created;
    expect(client).toBeInstanceOf(LatLngWasmClient);
    client.close();
    expect(worker.terminated).toBe(true);
  });

  it("passes an explicit wasm URL to worker initialization", async () => {
    const worker = new FakeWorker();
    const created = createLatLng({
      wasmUrl: "https://cdn.example.test/latlng_core_bg.wasm",
      workerFactory: () => worker as unknown as Worker,
    });

    expect(worker.requests[0]).toMatchObject({
      method: "init",
      params: ["https://cdn.example.test/latlng_core_bg.wasm"],
    });
    worker.reply(worker.requests[0]!.id, {});
    const client = await created;
    client.close();
  });

  it("does not expose worker storage internals on the public client", () => {
    const worker = new FakeWorker();
    const client = new LatLngWasmClient(worker as unknown as Worker) as unknown as Record<
      string,
      unknown
    >;

    expect(client.apply_storage_entries).toBeUndefined();
    expect(client.decode_storage_entry).toBeUndefined();
    expect(client.reset_state).toBeUndefined();
    expect(client.last_sequence).toBeUndefined();
  });
});
