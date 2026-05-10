import { describe, expect, it } from "vitest";

import { LatLngClient } from "../../src/index.js";

class FakeWebSocket extends EventTarget {
  public readonly sent: unknown[] = [];
  public readonly url: string;
  public readyState = WebSocket.OPEN;

  public constructor(url: string) {
    super();
    this.url = url;
    queueMicrotask(() => {
      this.dispatchEvent(new Event("open"));
    });
  }

  public send(payload: string): void {
    const parsed = JSON.parse(payload) as Record<string, unknown>;
    this.sent.push(parsed);
    if (parsed.type === "auth") {
      this.dispatchMessage({ ok: true, authorized: true });
      return;
    }
    if (parsed.type === "subscribe") {
      this.dispatchMessage({ ok: true, subscribed: parsed.channels });
      queueMicrotask(() => {
        this.dispatchMessage({
          command: "Set",
          detect: "Enter",
          collection: "fleet",
          id: "truck-1",
          object: { Point: { lat: 52.52, lon: 13.405, z: null } },
          fields: { values: {} },
          timestamp_ns: 123,
          event_id: "evt-1",
          job_id: null,
          hook: null,
          group: "fleet-events",
          nearby: null,
        });
      });
    }
    if (parsed.type === "quit") {
      queueMicrotask(() => {
        this.close();
      });
    }
  }

  public close(): void {
    this.readyState = WebSocket.CLOSED;
    this.dispatchEvent(new Event("close"));
  }

  private dispatchMessage(payload: unknown): void {
    this.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify(payload),
      }),
    );
  }
}

describe("LatLngWebSocketClient", () => {
  it("authenticates and streams subscription events", async () => {
    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      token: "secret",
      webSocketFactory: ((url: string) => new FakeWebSocket(url) as unknown as WebSocket),
    });

    const socket = await client.connectWebSocket();
    const subscription = await socket.subscribe(["fleet-events"]);
    const iterator = subscription[Symbol.asyncIterator]();
    const event = (await iterator.next()).value!;

    expect(event.collection).toBe("fleet");
    expect(event.detect).toBe("enter");
    expect(event.object.type).toBe("point");

    await socket.quit();
  });
});
