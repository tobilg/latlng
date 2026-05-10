import { withBearerToken } from "../auth/token.js";
import {
  AuthError,
  HttpError,
  ServerUnavailableError,
  TimeoutError,
} from "../errors/index.js";
import { parseJsonResponse } from "../internal/json.js";
import { joinUrl } from "./routes.js";

export interface HttpTransportConfig {
  baseUrl: string;
  token?: string;
  timeoutMs?: number;
  headers?: HeadersInit;
  fetch?: typeof globalThis.fetch;
}

export interface RequestOptions<T> {
  method?: "GET" | "POST" | "DELETE";
  path: string;
  body?: unknown;
  baseUrl?: string;
  parser?: (value: unknown) => T;
}

export class HttpTransport {
  private readonly baseUrl: string;
  private readonly token?: string;
  private readonly timeoutMs: number;
  private readonly headers?: HeadersInit;
  private readonly fetchImpl: typeof globalThis.fetch;

  public constructor(config: HttpTransportConfig) {
    this.baseUrl = config.baseUrl;
    this.token = config.token;
    this.timeoutMs = config.timeoutMs ?? 5_000;
    this.headers = config.headers;
    this.fetchImpl = config.fetch ?? globalThis.fetch.bind(globalThis);
  }

  public async request<T>(options: RequestOptions<T>): Promise<T> {
    const url = joinUrl(options.baseUrl ?? this.baseUrl, options.path);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    const headers = new Headers(this.headers);
    if (options.body !== undefined) {
      headers.set("content-type", "application/json");
    }
    withBearerToken(headers, this.token);

    try {
      const response = await this.fetchImpl(url, {
        method: options.method ?? "GET",
        headers,
        body:
          options.body === undefined ? undefined : JSON.stringify(options.body),
        signal: controller.signal,
      });
      const payload = await parseJsonResponse(response);
      if (!response.ok) {
        throw this.toHttpError(url, response.status, payload);
      }
      return options.parser ? options.parser(payload) : (payload as T);
    } catch (error) {
      if (error instanceof HttpError) {
        throw error;
      }
      if (error instanceof DOMException && error.name === "AbortError") {
        throw new TimeoutError(this.timeoutMs, { cause: error });
      }
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  public async requestText(options: Omit<RequestOptions<string>, "parser">): Promise<string> {
    const url = joinUrl(options.baseUrl ?? this.baseUrl, options.path);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    const headers = new Headers(this.headers);
    if (options.body !== undefined) {
      headers.set("content-type", "application/json");
    }
    withBearerToken(headers, this.token);

    try {
      const response = await this.fetchImpl(url, {
        method: options.method ?? "GET",
        headers,
        body:
          options.body === undefined ? undefined : JSON.stringify(options.body),
        signal: controller.signal,
      });
      const payload = await response.text();
      if (!response.ok) {
        throw this.toHttpError(url, response.status, payload);
      }
      return payload;
    } catch (error) {
      if (error instanceof HttpError) {
        throw error;
      }
      if (error instanceof DOMException && error.name === "AbortError") {
        throw new TimeoutError(this.timeoutMs, { cause: error });
      }
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  private toHttpError(
    url: string,
    status: number,
    payload: unknown,
  ): HttpError {
    const message =
      payload &&
      typeof payload === "object" &&
      "error" in payload &&
      typeof payload.error === "string"
        ? payload.error
        : typeof payload === "string" && payload.length > 0
          ? payload
          : `request failed with status ${status}`;
    const options = { status, url, body: payload };
    if (status === 401) {
      return new AuthError(message, options);
    }
    if (status === 503) {
      return new ServerUnavailableError(message, options);
    }
    return new HttpError(message, options);
  }
}
