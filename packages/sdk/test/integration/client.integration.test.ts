import { afterEach, describe, expect, it } from "vitest";

import {
  type GeofenceEvent,
  geojson,
  LatLngClient,
  point,
  ServerUnavailableError,
} from "../../src/index.js";
import {
  freePort,
  makeTempDir,
  startServer,
  waitForFollowerCaughtUp,
  waitForObjectPresent,
  writeServerConfig,
  type StartedServer,
} from "./helpers.js";

const started: StartedServer[] = [];

afterEach(async () => {
  while (started.length > 0) {
    const server = started.pop();
    await server?.stop();
  }
});

describe("latlng TypeScript SDK integration", () => {
  it("supports HTTP CRUD, query, admin, hooks, and channels", async () => {
    const dir = await makeTempDir("http");
    const httpPort = await freePort();
    const capnpPort = await freePort();
    const configPath = `${dir}/latlng.json`;
    await writeServerConfig(configPath, {
      httpPort,
      capnpPort,
      aofPath: `${dir}/appendonly.aof`,
      serverId: "leader-http",
      bearerToken: "secret",
    });

    const server = await startServer(configPath);
    started.push(server);

    const client = new LatLngClient({
      leaderUrl: server.baseUrl,
      token: "secret",
    });

    expect(await client.createCollection("fleet")).toBe(true);
    expect(await client.createCollection("fleet")).toBe(false);
    await expect(client.getCollection("fleet")).resolves.toMatchObject({
      name: "fleet",
      stats: {
        object_count: 0,
      },
    });

    await client.setObject("fleet", "truck-1", point(52.52, 13.405), {
      fields: { status: { type: "text", value: "moving" } },
    });

    const object = await client.get("fleet", "truck-1", { withFields: true });
    expect(object?.id).toBe("truck-1");
    expect(object?.geo.type).toBe("point");
    expect(object?.fields.status?.value).toBe("moving");

    const nearby = await client.nearby("fleet", {
      lat: 52.52,
      lon: 13.405,
      meters: 500,
    });
    expect(nearby.results[0]?.id).toBe("truck-1");

    expect(await client.delete("fleet", "truck-1")).toBe(true);
    await expect(client.getCollection("fleet")).resolves.toMatchObject({
      name: "fleet",
      stats: {
        object_count: 0,
      },
    });

    await client.setObject("fleet", "truck-1", point(52.52, 13.405), {
      fields: { status: { type: "text", value: "moving" } },
    });

    await client.setFields("fleet", "truck-1", {
      speed: { type: "number", value: 42 },
    });
    expect((await client.getField("fleet", "truck-1", "speed"))?.value).toBe(42);

    await client.setObject(
      "fleet",
      "truck-json",
      geojson({
        type: "Feature",
        geometry: {
          type: "Point",
          coordinates: [13.405, 52.52],
        },
        properties: {},
      }),
    );

    await client.setJson(
      "fleet",
      "truck-json",
      "properties.speed",
      JSON.stringify(42),
      { raw: true },
    );
    expect(await client.getJson("fleet", "truck-json", "properties.speed")).toBe(42);

    const timeout = await client.timeout({ command: "set", seconds: 1.5 });
    expect(timeout).toBe(1.5);

    expect(await client.configGet("readonly")).toBe("false");
    await client.configSet("readonly", "true");
    expect(await client.configGet("readonly")).toBe("true");
    await client.configSet("readonly", "false");

    await client.setChannel({
      name: "fleet-events",
      def: {
        collection: "fleet",
        query: {
          type: "nearby",
          lat: 52.52,
          lon: 13.405,
          meters: 500,
        },
        detect: ["enter"],
        commands: ["set"],
      },
    });
    expect(await client.channels()).toContain("fleet-events");
    await expect(client.getChannel("fleet-events")).resolves.toMatchObject({
      name: "fleet-events",
      def: {
        collection: "fleet",
        query: { type: "nearby", meters: 500 },
        detect: ["enter"],
        commands: ["set"],
      },
    });

    await client.setHook({
      name: "fleet-hook",
      endpoint: "https://example.com/hook",
      def: {
        collection: "fleet",
        query: {
          type: "nearby",
          lat: 52.52,
          lon: 13.405,
          meters: 500,
        },
        detect: ["enter"],
        commands: ["set"],
      },
    });
    expect((await client.hooks()).map((hook) => hook.name)).toContain("fleet-hook");
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

    const serverInfo = await client.server();
    expect(serverInfo.server_id).toBe("leader-http");

    const queue = await client.webhookQueue();
    expect(queue.pending).toBeGreaterThanOrEqual(0);
  });

  it("streams geofence events over WebSocket", async () => {
    const dir = await makeTempDir("ws");
    const httpPort = await freePort();
    const capnpPort = await freePort();
    const configPath = `${dir}/latlng.json`;
    await writeServerConfig(configPath, {
      httpPort,
      capnpPort,
      aofPath: `${dir}/appendonly.aof`,
      serverId: "leader-ws",
      bearerToken: "secret",
    });

    const server = await startServer(configPath);
    started.push(server);

    const client = new LatLngClient({
      leaderUrl: server.baseUrl,
      token: "secret",
    });

    await client.setChannel({
      name: "fleet-events",
      def: {
        collection: "fleet",
        query: {
          type: "nearby",
          lat: 52.52,
          lon: 13.405,
          meters: 500,
        },
        detect: ["enter"],
        commands: ["set"],
      },
    });

    const socket = await client.connectWebSocket();
    const subscription = await socket.subscribe(["fleet-events"]);
    const eventPromise = new Promise<GeofenceEvent>((resolve) => {
      subscription.on("event", resolve);
    });

    await client.setObject("fleet", "truck-1", point(52.52, 13.405));

    const event = await eventPromise;
    expect(event.collection).toBe("fleet");
    expect(event.id).toBe("truck-1");
    expect(event.detect).toBe("enter");

    await socket.quit();
  });

  it("streams geofence events over WebSocket pattern subscriptions", async () => {
    const dir = await makeTempDir("ws-pattern");
    const httpPort = await freePort();
    const capnpPort = await freePort();
    const configPath = `${dir}/latlng.json`;
    await writeServerConfig(configPath, {
      httpPort,
      capnpPort,
      aofPath: `${dir}/appendonly.aof`,
      serverId: "leader-ws-pattern",
      bearerToken: "secret",
    });

    const server = await startServer(configPath);
    started.push(server);

    const client = new LatLngClient({
      leaderUrl: server.baseUrl,
      token: "secret",
    });

    await client.setChannel({
      name: "fleet-pattern-events",
      def: {
        collection: "fleet",
        query: {
          type: "nearby",
          lat: 52.52,
          lon: 13.405,
          meters: 500,
        },
        detect: ["enter"],
        commands: ["set"],
      },
    });

    const socket = await client.connectWebSocket();
    const subscription = await socket.psubscribe(["fleet-*"]);
    const eventPromise = new Promise<GeofenceEvent>((resolve) => {
      subscription.on("event", resolve);
    });

    await client.setObject("fleet", "truck-2", point(52.52, 13.405));

    const event = await eventPromise;
    expect(event.collection).toBe("fleet");
    expect(event.id).toBe("truck-2");
    expect(event.detect).toBe("enter");

    await socket.quit();
  });

  it("rejects follower reads before caught_up_once", async () => {
    const dir = await makeTempDir("replication-catchup");
    const leaderCapnpPort = await freePort();
    const followerHttpPort = await freePort();
    const followerCapnpPort = await freePort();
    const credential = "replication-secret";

    const followerConfig = `${dir}/follower.json`;

    await writeServerConfig(followerConfig, {
      httpPort: followerHttpPort,
      capnpPort: followerCapnpPort,
      aofPath: `${dir}/follower.aof`,
      serverId: "follower-catchup",
      bearerToken: "secret",
      replicationCredential: credential,
      follow: { host: "127.0.0.1", port: leaderCapnpPort },
    });

    const follower = await startServer(followerConfig);
    started.push(follower);

    const client = new LatLngClient({
      leaderUrl: follower.baseUrl,
      token: "secret",
    });

    await expect(client.get("fleet", "truck-1")).rejects.toBeInstanceOf(
      ServerUnavailableError,
    );
  });

  it("can read from a caught-up follower when the leader is unavailable", async () => {
    const dir = await makeTempDir("replication");
    const leaderHttpPort = await freePort();
    const leaderCapnpPort = await freePort();
    const followerHttpPort = await freePort();
    const followerCapnpPort = await freePort();
    const credential = "replication-secret";

    const leaderConfig = `${dir}/leader.json`;
    const followerConfig = `${dir}/follower.json`;

    await writeServerConfig(leaderConfig, {
      httpPort: leaderHttpPort,
      capnpPort: leaderCapnpPort,
      aofPath: `${dir}/leader.aof`,
      serverId: "leader-1",
      bearerToken: "secret",
      replicationCredential: credential,
    });
    await writeServerConfig(followerConfig, {
      httpPort: followerHttpPort,
      capnpPort: followerCapnpPort,
      aofPath: `${dir}/follower.aof`,
      serverId: "follower-1",
      bearerToken: "secret",
      replicationCredential: credential,
      follow: { host: "127.0.0.1", port: leaderCapnpPort },
    });

    const leader = await startServer(leaderConfig);
    started.push(leader);
    const follower = await startServer(followerConfig);
    started.push(follower);

    const leaderClient = new LatLngClient({
      leaderUrl: leader.baseUrl,
      token: "secret",
    });

    await leaderClient.setObject("fleet", "truck-1", point(52.52, 13.405));
    await waitForFollowerCaughtUp(follower.baseUrl);
    await waitForObjectPresent(follower.baseUrl, "fleet", "truck-1");

    const replicaAware = new LatLngClient({
      leaderUrl: leader.baseUrl,
      readReplicas: [follower.baseUrl],
      readPreference: "followerPreferred",
      token: "secret",
    });

    const before = await replicaAware.get("fleet", "truck-1");
    expect(before?.id).toBe("truck-1");

    await leader.stop();
    const leaderIndex = started.indexOf(leader);
    if (leaderIndex >= 0) {
      started.splice(leaderIndex, 1);
    }

    const after = await replicaAware.get("fleet", "truck-1");
    expect(after?.id).toBe("truck-1");
  });

});
