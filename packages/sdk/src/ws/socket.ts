import { LatLngError } from "../errors/index.js";
import { type GeofenceEvent, parseGeofenceEvent } from "../types/events.js";
import { LatLngSubscription } from "./subscriptions.js";

/**
 * Configuration for establishing a WebSocket session.
 */
export interface LatLngWebSocketConfig {
  /** Full WebSocket endpoint URL, for example `ws://127.0.0.1:7421/ws`. */
  url: string;
  /** Optional bearer token or JWT sent through the initial `auth` command. */
  token?: string;
  /** Custom WebSocket constructor/factory for tests or non-browser runtimes. */
  webSocketFactory?: (url: string) => WebSocket;
}

type PendingResolver = {
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
};

/**
 * WebSocket client for `latlng` channel subscriptions.
 */
export class LatLngWebSocketClient {
  private readonly socket: WebSocket;
  private readonly token?: string;
  private readonly pending: PendingResolver[] = [];
  private activeSubscription?: LatLngSubscription;

  private constructor(socket: WebSocket, token?: string) {
    this.socket = socket;
    this.token = token;
    this.socket.addEventListener("message", (event) => {
      this.handleMessage(event);
    });
    this.socket.addEventListener("close", () => {
      this.activeSubscription?.finish();
      this.activeSubscription = undefined;
      while (this.pending.length > 0) {
        this.pending.shift()?.reject(
          new LatLngError("websocket closed before command acknowledgement"),
        );
      }
    });
    this.socket.addEventListener("error", () => {
      this.activeSubscription?.fail(new LatLngError("websocket transport error"));
    });
  }

  /**
   * Opens a WebSocket connection and optionally authenticates it with the provided token.
   *
   * @param config WebSocket connection configuration.
   * @returns Connected WebSocket client.
   */
  public static async connect(
    config: LatLngWebSocketConfig,
  ): Promise<LatLngWebSocketClient> {
    const factory =
      config.webSocketFactory ??
      ((url: string) => {
        if (typeof WebSocket !== "function") {
          throw new LatLngError("global WebSocket is not available");
        }
        return new WebSocket(url);
      });
    const socket = factory(config.url);
    await new Promise<void>((resolve, reject) => {
      socket.addEventListener("open", () => resolve(), { once: true });
      socket.addEventListener(
        "error",
        () => reject(new LatLngError("failed to open websocket")),
        { once: true },
      );
    });

    const client = new LatLngWebSocketClient(socket, config.token);
    if (config.token) {
      const auth = await client.command({
        type: "auth",
        token: config.token,
      });
      if (!auth || typeof auth !== "object" || (auth as { authorized?: boolean }).authorized !== true) {
        throw new LatLngError("websocket authentication failed");
      }
    }
    return client;
  }

  /**
   * Sends a protocol-level ping command over the active WebSocket connection.
   *
   * @returns Raw ping acknowledgement payload.
   */
  public async ping(): Promise<unknown> {
    return this.command({ type: "ping" });
  }

  /**
   * Subscribes to one or more explicit channel names.
   *
   * The returned subscription represents the active stream for this socket.
   *
   * @param channels Channel names to subscribe to.
   * @returns Active subscription stream.
   */
  public async subscribe(channels: string[]): Promise<LatLngSubscription> {
    const subscription = this.replaceSubscription();
    try {
      await this.command({ type: "subscribe", channels });
      return subscription;
    } catch (error) {
      subscription.close();
      throw error;
    }
  }

  /**
   * Subscribes to one or more channel patterns.
   *
   * The returned subscription represents the active stream for this socket.
   *
   * @param patterns Channel patterns to subscribe to.
   * @returns Active subscription stream.
   */
  public async psubscribe(patterns: string[]): Promise<LatLngSubscription> {
    const subscription = this.replaceSubscription();
    try {
      await this.command({ type: "psubscribe", patterns });
      return subscription;
    } catch (error) {
      subscription.close();
      throw error;
    }
  }

  /**
   * Requests a graceful server-side close and then closes the underlying socket.
   */
  public async quit(): Promise<void> {
    try {
      await this.command({ type: "quit" });
    } catch {
      // The server may close immediately before acknowledging quit.
    } finally {
      this.close();
    }
  }

  /**
   * Closes the underlying WebSocket immediately.
   */
  public close(): void {
    this.socket.close();
  }

  private replaceSubscription(): LatLngSubscription {
    this.activeSubscription?.close();
    const subscription = new LatLngSubscription(() => {
      this.close();
    });
    this.activeSubscription = subscription;
    return subscription;
  }

  private async command(payload: unknown): Promise<unknown> {
    if (this.socket.readyState !== WebSocket.OPEN) {
      throw new LatLngError("websocket is not open");
    }
    const promise = new Promise<unknown>((resolve, reject) => {
      this.pending.push({ resolve, reject });
    });
    this.socket.send(JSON.stringify(payload));
    return promise;
  }

  private handleMessage(event: MessageEvent<string>): void {
    const payload = JSON.parse(event.data) as unknown;
    const parsedEvent = this.tryParseEvent(payload);
    if (parsedEvent) {
      this.activeSubscription?.push(parsedEvent);
      return;
    }

    const next = this.pending.shift();
    if (!next) {
      return;
    }
    if (
      payload &&
      typeof payload === "object" &&
      "error" in payload &&
      typeof (payload as { error?: unknown }).error === "string"
    ) {
      next.reject(new LatLngError((payload as { error: string }).error));
      return;
    }
    next.resolve(payload);
  }

  private tryParseEvent(payload: unknown): GeofenceEvent | null {
    try {
      return parseGeofenceEvent(payload);
    } catch {
      return null;
    }
  }
}
