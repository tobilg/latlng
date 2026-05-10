export function withBearerToken(
  headers: Headers,
  token: string | undefined,
): void {
  if (token) {
    headers.set("authorization", `Bearer ${token}`);
  }
}
