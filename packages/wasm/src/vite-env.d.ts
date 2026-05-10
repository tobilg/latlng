/// <reference types="vite/client" />

declare module "../pkg/latlng_core.js" {
  export default function init(input?: RequestInfo | URL | Response | BufferSource): Promise<void>;

  export class BrowserLatLng {
    constructor();
    create_collection(collection: string): unknown;
    drop_collection(collection: string): unknown;
    collections(pattern: string | null): unknown;
    collection_info(collection: string): unknown;
    set_object(collection: string, id: string, request: unknown): unknown;
    get_object(collection: string, id: string, withFields: boolean): unknown;
    delete_object(collection: string, id: string): unknown;
    expire(collection: string, id: string, seconds: number): unknown;
    persist(collection: string, id: string): unknown;
    ttl(collection: string, id: string): unknown;
    set_hook(name: string, request: unknown): void;
    delete_hook(name: string): boolean;
    hooks(pattern: string | null): unknown;
    hook(name: string): unknown;
    nearby_query(collection: string, query: unknown): unknown;
    within_query(collection: string, query: unknown): unknown;
    intersects_query(collection: string, query: unknown): unknown;
    scan(collection: string, options: unknown): unknown;
    search(collection: string, options: unknown): unknown;
    bounds(collection: string): unknown;
    stats(collections: unknown): unknown;
    server_info(): unknown;
  }
}
