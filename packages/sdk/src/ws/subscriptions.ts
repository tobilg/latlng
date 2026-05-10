import type { GeofenceEvent } from "../types/events.js";

class AsyncQueue<T> {
  private values: T[] = [];
  private resolvers: Array<(value: IteratorResult<T>) => void> = [];
  private closed = false;

  public push(value: T): void {
    if (this.closed) {
      return;
    }
    const resolver = this.resolvers.shift();
    if (resolver) {
      resolver({ value, done: false });
      return;
    }
    this.values.push(value);
  }

  public close(): void {
    this.closed = true;
    while (this.resolvers.length > 0) {
      const resolver = this.resolvers.shift();
      resolver?.({ value: undefined, done: true });
    }
  }

  public next(): Promise<IteratorResult<T>> {
    const value = this.values.shift();
    if (value !== undefined) {
      return Promise.resolve({ value, done: false });
    }
    if (this.closed) {
      return Promise.resolve({ value: undefined, done: true });
    }
    return new Promise((resolve) => {
      this.resolvers.push(resolve);
    });
  }
}

/**
 * Event stream wrapper returned by WebSocket subscription commands.
 *
 * It supports callback-style listeners and async iteration.
 */
export class LatLngSubscription implements AsyncIterable<GeofenceEvent> {
  private readonly events = new AsyncQueue<GeofenceEvent>();
  private readonly eventHandlers = new Set<(event: GeofenceEvent) => void>();
  private readonly closeHandlers = new Set<() => void>();
  private readonly errorHandlers = new Set<(error: unknown) => void>();
  private readonly closeFn: () => void;
  private closed = false;

  /**
   * Creates a subscription wrapper around the underlying socket close callback.
   *
   * @param closeFn Callback used to close the underlying socket transport.
   */
  public constructor(closeFn: () => void) {
    this.closeFn = closeFn;
  }

  /**
   * Registers an event listener.
   *
   * Returns an unsubscribe function that removes the handler again.
   *
   * @param type Event type to subscribe to.
   * @param handler Event handler callback.
   * @returns Function that removes the registered handler.
   */
  public on(type: "event", handler: (event: GeofenceEvent) => void): () => void;
  public on(type: "close", handler: () => void): () => void;
  public on(type: "error", handler: (error: unknown) => void): () => void;
  public on(
    type: "event" | "close" | "error",
    handler:
      | ((event: GeofenceEvent) => void)
      | (() => void)
      | ((error: unknown) => void),
  ): () => void {
    if (type === "event") {
      this.eventHandlers.add(handler as (event: GeofenceEvent) => void);
      return () => {
        this.eventHandlers.delete(handler as (event: GeofenceEvent) => void);
      };
    }
    if (type === "close") {
      this.closeHandlers.add(handler as () => void);
      return () => {
        this.closeHandlers.delete(handler as () => void);
      };
    }
    this.errorHandlers.add(handler as (error: unknown) => void);
    return () => {
      this.errorHandlers.delete(handler as (error: unknown) => void);
    };
  }

  public push(event: GeofenceEvent): void {
    this.events.push(event);
    for (const handler of this.eventHandlers) {
      handler(event);
    }
  }

  public fail(error: unknown): void {
    for (const handler of this.errorHandlers) {
      handler(error);
    }
  }

  /**
   * Closes the subscription and its underlying socket.
   */
  public close(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.closeFn();
    this.finish();
  }

  public finish(): void {
    if (!this.closed) {
      this.closed = true;
    }
    this.events.close();
    for (const handler of this.closeHandlers) {
      handler();
    }
  }

  /**
   * Returns an async iterator that yields geofence events until the subscription closes.
   *
   * @returns Async iterator over geofence events.
   */
  public [Symbol.asyncIterator](): AsyncIterator<GeofenceEvent> {
    return {
      next: () => this.events.next(),
    };
  }
}
