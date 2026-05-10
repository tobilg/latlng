import { once } from "node:events";
import { promises as fs } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { tmpdir } from "node:os";
import { setTimeout as sleep } from "node:timers/promises";

export interface StartedServer {
  child: ChildProcessWithoutNullStreams;
  baseUrl: string;
  capnpUrl: string;
  stop: () => Promise<void>;
}

const repoRoot = resolve(new URL("../../../../", import.meta.url).pathname);

export async function freePort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolvePort, reject) => {
    server.listen(0, "127.0.0.1", () => resolvePort());
    server.once("error", reject);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((resolveClose, reject) => {
    server.close((error) => (error ? reject(error) : resolveClose()));
  });
  return port;
}

export async function makeTempDir(name: string): Promise<string> {
  const dir = join(tmpdir(), `latlng-ts-sdk-${name}-${Date.now()}-${Math.random().toString(16).slice(2)}`);
  await fs.mkdir(dir, { recursive: true });
  return dir;
}

export async function writeServerConfig(
  path: string,
  options: {
    httpPort: number;
    capnpPort: number;
    aofPath: string;
    serverId?: string;
    bearerToken?: string;
    replicationCredential?: string;
    follow?: { host: string; port: number };
  },
): Promise<void> {
  const payload = {
    listen_addr: `127.0.0.1:${options.httpPort}`,
    capnp_enabled: true,
    capnp_listen_addr: `127.0.0.1:${options.capnpPort}`,
    server_id: options.serverId,
    storage: {
      type: "aof",
      path: options.aofPath,
    },
    bearer_token: options.bearerToken ?? "secret",
    replication_credential: options.replicationCredential,
    follow_host: options.follow?.host,
    follow_port: options.follow?.port,
    replication_batch_size: 32,
    replication_reconnect_backoff_ms: 50,
  };
  await fs.writeFile(path, JSON.stringify(payload, null, 2), "utf8");
}

export async function startServer(configPath: string): Promise<StartedServer> {
  const binary = resolve(repoRoot, "target/debug/latlng-server");
  const command = await exists(binary)
    ? {
        cmd: binary,
        args: ["--config", configPath],
      }
    : {
        cmd: "cargo",
        args: ["run", "-p", "latlng-server", "--", "--config", configPath],
      };

  const child = spawn(command.cmd, command.args, {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });

  const logs = { text: "" };
  child.stdout.on("data", (chunk) => {
    logs.text += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    logs.text += chunk.toString();
  });

  const baseUrl = await waitForBaseUrl(configPath, logs, child);
  const config = JSON.parse(await fs.readFile(configPath, "utf8")) as {
    capnp_listen_addr: string;
  };

  return {
    child,
    baseUrl,
    capnpUrl: `tcp://${config.capnp_listen_addr}`,
    stop: async () => {
      child.kill("SIGTERM");
      await Promise.race([
        once(child, "exit").then(() => undefined),
        sleep(5_000).then(() => {
          if (!child.killed) {
            child.kill("SIGKILL");
          }
        }),
      ]);
    },
  };
}

export async function waitForPing(baseUrl: string, token = "secret"): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    try {
      const response = await fetch(`${baseUrl}/ping`, {
        headers: { authorization: `Bearer ${token}` },
      });
      if (response.ok) {
        return;
      }
    } catch {
      // keep polling
    }
    await sleep(250);
  }
  throw new Error(`server at ${baseUrl} did not become ready`);
}

export async function waitForFollowerCaughtUp(
  baseUrl: string,
  token = "secret",
): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const response = await fetch(`${baseUrl}/server`, {
      headers: { authorization: `Bearer ${token}` },
    });
    if (response.ok) {
      const json = (await response.json()) as {
        leader?: boolean;
        caught_up_once?: boolean;
      };
      if (json.leader === false && json.caught_up_once === true) {
        return;
      }
    }
    await sleep(250);
  }
  throw new Error(`follower at ${baseUrl} did not catch up`);
}

export async function waitForObjectPresent(
  baseUrl: string,
  collection: string,
  id: string,
  token = "secret",
): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const response = await fetch(
      `${baseUrl}/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}`,
      {
        headers: { authorization: `Bearer ${token}` },
      },
    );
    if (response.ok) {
      return;
    }
    await sleep(250);
  }
  throw new Error(`object ${collection}/${id} not visible at ${baseUrl}`);
}

async function exists(path: string): Promise<boolean> {
  try {
    await fs.stat(path);
    return true;
  } catch {
    return false;
  }
}

async function waitForBaseUrl(
  configPath: string,
  logs: { text: string },
  child: ChildProcessWithoutNullStreams,
): Promise<string> {
  const config = JSON.parse(await fs.readFile(configPath, "utf8")) as {
    listen_addr: string;
    bearer_token?: string;
  };
  const baseUrl = `http://${config.listen_addr}`;

  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (child.exitCode !== null) {
      throw new Error(`server exited early:\n${logs.text}`);
    }
    try {
      const response = await fetch(`${baseUrl}/ping`, {
        headers: {
          authorization: `Bearer ${config.bearer_token ?? "secret"}`,
        },
      });
      if (response.ok) {
        return baseUrl;
      }
    } catch {
      // keep polling
    }
    await sleep(250);
  }

  throw new Error(`server did not start:\n${logs.text}`);
}
