export type { JsonPrimitive, JsonValue } from "../types/json.js";

export async function parseJsonResponse(
  response: Response,
): Promise<unknown | undefined> {
  if (response.status === 204) {
    return undefined;
  }

  const text = await response.text();
  if (!text) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}
