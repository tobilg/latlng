/** Primitive JSON value supported by `latlng` request and response payloads. */
export type JsonPrimitive = string | number | boolean | null;

/** Recursive JSON value supported by `latlng` request and response payloads. */
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
