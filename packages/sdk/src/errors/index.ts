/**
 * Base SDK error type.
 */
export class LatLngError extends Error {
  /** Optional underlying cause. */
  public readonly cause?: unknown;

  /**
   * Creates a new SDK error.
   *
   * @param message Human-readable error message.
   * @param options Additional error metadata.
   */
  public constructor(message: string, options?: { cause?: unknown }) {
    super(message);
    this.name = new.target.name;
    this.cause = options?.cause;
  }
}

/**
 * Error raised when an HTTP request exceeds the configured timeout.
 */
export class TimeoutError extends LatLngError {
  /** Configured timeout in milliseconds. */
  public readonly timeoutMs: number;

  /**
   * Creates a timeout error.
   *
   * @param timeoutMs Timeout that was exceeded.
   * @param options Additional error metadata.
   */
  public constructor(timeoutMs: number, options?: { cause?: unknown }) {
    super(`request timed out after ${timeoutMs}ms`, options);
    this.timeoutMs = timeoutMs;
  }
}

/**
 * Error raised for non-success HTTP responses.
 */
export class HttpError extends LatLngError {
  /** HTTP status code returned by the server. */
  public readonly status: number;
  /** Request URL that produced the error. */
  public readonly url: string;
  /** Parsed response body, when available. */
  public readonly body?: unknown;

  /**
   * Creates an HTTP error wrapper.
   *
   * @param message Human-readable error message.
   * @param options HTTP error metadata.
   */
  public constructor(
    message: string,
    options: { status: number; url: string; body?: unknown; cause?: unknown },
  ) {
    super(message, options);
    this.status = options.status;
    this.url = options.url;
    this.body = options.body;
  }
}

/**
 * HTTP error raised for authentication failures.
 */
export class AuthError extends HttpError {}

/**
 * HTTP error raised when the server is temporarily unavailable.
 */
export class ServerUnavailableError extends HttpError {}
