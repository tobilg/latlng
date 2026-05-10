export function joinUrl(baseUrl: string, path: string): string {
  const base = baseUrl.endsWith("/") ? baseUrl.slice(0, -1) : baseUrl;
  return `${base}${path}`;
}

export function buildQuery(
  query: Record<string, string | number | boolean | null | undefined>,
): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) {
      continue;
    }
    params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : "";
}

export const routes = {
  ping: () => "/ping",
  healthz: () => "/healthz",
  server: () => "/server",
  info: (section?: string) => `/info${buildQuery({ section })}`,
  metrics: () => "/metrics",
  collections: (matchPattern?: string) =>
    `/collections${buildQuery({ match_pattern: matchPattern })}`,
  collection: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}`,
  renameCollection: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/rename`,
  bounds: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/bounds`,
  stats: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/stats`,
  object: (collection: string, id: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}`,
  objects: (collection: string, matchPattern?: string) =>
    `/collections/${encodeURIComponent(collection)}/objects${buildQuery({
      match_pattern: matchPattern,
    })}`,
  fields: (collection: string, id: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/fields`,
  field: (collection: string, id: string, field: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/fields/${encodeURIComponent(field)}`,
  expire: (collection: string, id: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/expire`,
  ttl: (collection: string, id: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/ttl`,
  jsonRoot: (collection: string, id: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/json`,
  jsonPath: (collection: string, id: string, path: string) =>
    `/collections/${encodeURIComponent(collection)}/objects/${encodeURIComponent(id)}/json/${path}`,
  nearby: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/search/nearby`,
  within: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/search/within`,
  intersects: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/search/intersects`,
  scan: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/search/scan`,
  search: (collection: string) =>
    `/collections/${encodeURIComponent(collection)}/search/text`,
  channels: (matchPattern?: string) =>
    `/channels${buildQuery({ match_pattern: matchPattern })}`,
  channel: (name: string) => `/channels/${encodeURIComponent(name)}`,
  hooks: (matchPattern?: string) =>
    `/hooks${buildQuery({ match_pattern: matchPattern })}`,
  hook: (name: string) => `/hooks/${encodeURIComponent(name)}`,
  config: (name: string) => `/config/${encodeURIComponent(name)}`,
  flushdb: () => "/admin/flushdb",
  gc: () => "/admin/gc",
  aofshrink: () => "/admin/aofshrink",
  webhookQueue: () => "/admin/webhook-queue",
  configRewrite: () => "/admin/config/rewrite",
  readonly: () => "/admin/readonly",
  timeout: () => "/admin/timeout",
  ws: () => "/ws",
};
