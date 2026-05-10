import { parseServerInfo, type ServerInfo } from "../types/models.js";
import type { ReadPreference } from "../types/requests.js";
import { routes } from "../http/routes.js";
import { HttpTransport } from "../http/transport.js";
import { chooseReplicaOrder } from "./read-preference.js";

interface CachedReplicaStatus {
  expiresAt: number;
  info?: ServerInfo;
}

export interface ReplicaRouterConfig {
  leaderUrl: string;
  readReplicas?: string[];
  readPreference?: ReadPreference;
  statusTtlMs?: number;
}

export class ReplicaRouter {
  private readonly leaderUrl: string;
  private readonly readReplicas: string[];
  private readonly readPreference: ReadPreference;
  private readonly statusTtlMs: number;
  private readonly statusCache = new Map<string, CachedReplicaStatus>();
  private roundRobinCursor = 0;

  public constructor(config: ReplicaRouterConfig) {
    this.leaderUrl = config.leaderUrl;
    this.readReplicas = config.readReplicas ?? [];
    this.readPreference = config.readPreference ?? "leader";
    this.statusTtlMs = config.statusTtlMs ?? 1_000;
  }

  public getLeaderUrl(): string {
    return this.leaderUrl;
  }

  public async getReadCandidates(transport: HttpTransport): Promise<string[]> {
    const replicas = await Promise.all(
      this.readReplicas.map(async (url) => ({
        url,
        eligible: await this.isEligibleReplica(url, transport),
      })),
    );

    const ordered = chooseReplicaOrder(
      replicas,
      this.readPreference,
      this.roundRobinCursor,
    );
    if (this.readPreference === "roundRobinFollowers" && ordered.length > 0) {
      this.roundRobinCursor += 1;
    }
    return ordered;
  }

  public getReadPreference(): ReadPreference {
    return this.readPreference;
  }

  private async isEligibleReplica(
    url: string,
    transport: HttpTransport,
  ): Promise<boolean> {
    const now = Date.now();
    const cached = this.statusCache.get(url);
    if (cached && cached.expiresAt > now) {
      return Boolean(cached.info && !cached.info.leader && cached.info.caught_up_once);
    }

    try {
      const info = await transport.request({
        baseUrl: url,
        path: routes.server(),
        parser: parseServerInfo,
      });
      this.statusCache.set(url, {
        expiresAt: now + this.statusTtlMs,
        info,
      });
      return !info.leader && info.caught_up_once;
    } catch {
      this.statusCache.set(url, { expiresAt: now + this.statusTtlMs });
      return false;
    }
  }
}
