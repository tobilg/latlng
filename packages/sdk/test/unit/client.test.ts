import { describe, expect, it, vi } from "vitest";

import {
  LatLngClient,
  LatLngError,
  point,
  textField,
} from "../../src/index.js";
import { routes } from "../../src/http/routes.js";

describe("LatLngClient HTTP behavior", () => {
  it("serializes setObject requests with auth and field values", async () => {
    const fetchMock = vi.fn(async () => {
      return new Response(JSON.stringify({ ok: true, stored: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      token: "secret",
      fetch: fetchMock as typeof fetch,
    });

    const stored = await client.setObject("fleet", "truck-1", point(52.52, 13.405), {
      fields: {
        status: textField("moving"),
      },
      condition: "nx",
      expireSeconds: 30,
    });

    expect(stored).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe("http://leader:7421/collections/fleet/objects/truck-1");

    const headers = new Headers(init?.headers);
    expect(headers.get("authorization")).toBe("Bearer secret");
    expect(headers.get("content-type")).toBe("application/json");

    const body = JSON.parse(String(init?.body));
    expect(body.object).toEqual({
      Point: { lat: 52.52, lon: 13.405, z: null },
    });
    expect(body.condition).toBe("Nx");
    expect(body.expire_seconds).toBe(30);
    expect(body.fields).toEqual([
      {
        name: "status",
        value: { type: "text", value: "moving" },
      },
    ]);
  });

  it("creates collections explicitly through the collection endpoint", async () => {
    const fetchMock = vi.fn(async () => {
      return new Response(JSON.stringify({ ok: true, created: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      token: "secret",
      fetch: fetchMock as typeof fetch,
    });

    const created = await client.createCollection("fleet");

    expect(created).toBe(true);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe("http://leader:7421/collections/fleet");
    expect(init?.method).toBe("POST");
  });

  it("returns Prometheus metrics as text", async () => {
    const metrics = [
      "# HELP latlng_http_requests_total Total HTTP requests handled by the native server.",
      "# TYPE latlng_http_requests_total counter",
      "latlng_http_requests_total 7",
      "",
    ].join("\n");
    const fetchMock = vi.fn(async () => {
      return new Response(metrics, {
        status: 200,
        headers: { "content-type": "text/plain; version=0.0.4" },
      });
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      token: "secret",
      fetch: fetchMock as typeof fetch,
    });

    await expect(client.metrics()).resolves.toBe(metrics);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(String(url)).toBe("http://leader:7421/metrics");
    expect(new Headers(init?.headers).get("authorization")).toBe("Bearer secret");
  });

  it("uses an eligible replica for followerPreferred reads", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "http://follower:7421/server") {
        return new Response(
          JSON.stringify({
            leader: false,
            caught_up_once: true,
            server_id: "follower-1",
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      if (url === "http://follower:7421/collections/fleet/objects/truck-1?with_fields=false&format=objects") {
        return new Response(
          JSON.stringify({
            id: "truck-1",
            geo: { Point: { lat: 52.52, lon: 13.405, z: null } },
            fields: { values: {} },
            expires_at: null,
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      throw new Error(`unexpected url ${url}`);
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      readReplicas: ["http://follower:7421"],
      readPreference: "followerPreferred",
      fetch: fetchMock as typeof fetch,
    });

    const object = await client.get("fleet", "truck-1");

    expect(object?.id).toBe("truck-1");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(String(fetchMock.mock.calls[0]![0])).toBe("http://follower:7421/server");
    expect(String(fetchMock.mock.calls[1]![0])).toContain("http://follower:7421/collections/fleet/objects/truck-1");
  });

  it("reads full hook and channel definitions from the native detail endpoints", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "http://leader:7421/channels/fleet-events") {
        return new Response(
          JSON.stringify({
            name: "fleet-events",
            def: {
              collection: "fleet",
              query: {
                Nearby: {
                  lat: 52.52,
                  lon: 13.405,
                  meters: 500,
                  options: {
                    cursor: 0,
                    limit: 100,
                    nofields: false,
                    match_pattern: null,
                    sort: "Asc",
                    where_filters: [],
                    where_in_filters: [],
                    where_expr_filters: [],
                    clip: false,
                    output: "Objects",
                  },
                },
              },
              detect: ["Enter"],
              commands: ["Set"],
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      if (url === "http://leader:7421/hooks/fleet-hook") {
        return new Response(
          JSON.stringify({
            name: "fleet-hook",
            endpoint: "https://example.com/hook",
            def: {
              collection: "fleet",
              query: {
                Nearby: {
                  lat: 52.52,
                  lon: 13.405,
                  meters: 500,
                  options: {
                    cursor: 0,
                    limit: 100,
                    nofields: false,
                    match_pattern: null,
                    sort: "Asc",
                    where_filters: [],
                    where_in_filters: [],
                    where_expr_filters: [],
                    clip: false,
                    output: "Objects",
                  },
                },
              },
              detect: ["Enter"],
              commands: ["Set"],
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      throw new Error(`unexpected url ${url}`);
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      fetch: fetchMock as typeof fetch,
    });

    await expect(client.getChannel("fleet-events")).resolves.toMatchObject({
      name: "fleet-events",
      def: {
        collection: "fleet",
        query: { type: "nearby", meters: 500 },
        detect: ["enter"],
        commands: ["set"],
      },
    });
    await expect(client.getHook("fleet-hook")).resolves.toMatchObject({
      name: "fleet-hook",
      endpoint: "https://example.com/hook",
      def: {
        collection: "fleet",
        query: { type: "nearby", meters: 500 },
        detect: ["enter"],
        commands: ["set"],
      },
    });
  });

  it("falls back from leader to replica on leaderPreferred reads when leader is unavailable", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "http://leader:7421/collections/fleet/objects/truck-1?with_fields=false&format=objects") {
        return new Response(JSON.stringify({ error: "catching up to leader" }), {
          status: 503,
          headers: { "content-type": "application/json" },
        });
      }
      if (url === "http://follower:7421/server") {
        return new Response(
          JSON.stringify({
            leader: false,
            caught_up_once: true,
            server_id: "follower-1",
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      if (url === "http://follower:7421/collections/fleet/objects/truck-1?with_fields=false&format=objects") {
        return new Response(
          JSON.stringify({
            id: "truck-1",
            geo: { Point: { lat: 52.52, lon: 13.405, z: null } },
            fields: { values: {} },
            expires_at: null,
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      throw new Error(`unexpected url ${url}`);
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      readReplicas: ["http://follower:7421"],
      readPreference: "leaderPreferred",
      fetch: fetchMock as typeof fetch,
    });

    const object = await client.get("fleet", "truck-1");

    expect(object?.id).toBe("truck-1");
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it("parses direct field values returned by fget", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (
        url === "http://leader:7421/collections/fleet/objects/truck-1" &&
        init?.method === "POST"
      ) {
        return new Response(JSON.stringify({ ok: true, stored: true }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      if (url === "http://leader:7421/collections/fleet/objects/truck-1/fields/speed") {
        return new Response(JSON.stringify({ type: "number", value: 42 }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      throw new Error(`unexpected url ${url}`);
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      fetch: fetchMock as typeof fetch,
    });

    await client.setPoint("fleet", "truck-1", point(52.52, 13.405));

    await expect(client.getField("fleet", "truck-1", "speed")).resolves.toEqual({
      type: "number",
      value: 42,
    });
  });

  it("parses jget JSON snippets into JavaScript values", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "http://leader:7421/collections/fleet/objects/truck-1/json/properties.speed") {
        return new Response(JSON.stringify({ value: "42" }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }
      throw new Error(`unexpected url ${url}`);
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      fetch: fetchMock as typeof fetch,
    });

    await expect(client.getJson("fleet", "truck-1", "properties.speed")).resolves.toBe(42);
  });

  it("surfaces invalid collections responses as SDK errors", async () => {
    const fetchMock = vi.fn(async () => {
      return new Response(JSON.stringify({ wrong: [] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    });

    const client = new LatLngClient({
      leaderUrl: "http://leader:7421",
      fetch: fetchMock as typeof fetch,
    });

    await expect(client.collections()).rejects.toBeInstanceOf(LatLngError);
  });

  it("does not expose hidden or internal HTTP routes", () => {
    const routeValues = Object.values(routes).map((route) => {
      if (typeof route !== "function") {
        return String(route);
      }
      try {
        return route("collection", "object", "path");
      } catch {
        return "";
      }
    });

    expect(Object.keys(routes)).not.toContain("test");
    expect(routeValues).not.toContain("/test");
    expect(routeValues).not.toContain("/admin/follow");
  });
});
