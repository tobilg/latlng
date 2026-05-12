# Release Checklist

Before cutting a release:

1. Run `cargo fmt --all`.
2. Run `cargo clippy --workspace --all-targets`.
3. Run `cargo test --workspace`.
4. Run `cargo test -p latlng-server --test server_smoke`.
5. Run `cargo check --target wasm32-unknown-unknown -p latlng-core --features wasm-bindings`.
6. Run `make sdk-install sdk-typecheck sdk-build sdk-docs sdk-test`.
7. Run `make wasm-install wasm-typecheck wasm-build wasm-test wasm-pack-dry-run`.
8. Run `make docker-build-prebuilt` and verify the image starts with a mounted config file.
9. Run `npm audit --audit-level=high` in `packages/sdk` and `packages/wasm`.
10. Run `cargo run -p latlng-benchmark` and capture the output.
11. Run `make bench-server BENCH_FLAGS="--warmup-secs 1 --measure-secs 2 --seed-objects 1000 --startup-records 1000"` and save the JSON output from `benchmark-results/`.
12. Run `make openapi` and inspect `dist/openapi.json`.
13. Verify release binary archives contain only `latlng-server` and `latlng-cli`; benchmark binaries must stay local-only.
14. Verify `README.md`, `docs/architecture.md`, `docs/auth.md`, `docs/config.md`, `packages/sdk/README.md`, and `packages/wasm/README.md` still match the shipped surface.
15. Confirm the generated Cap'n Proto code is up to date after schema changes.
16. Review production deployment guidance for upstream TLS termination and edge per-client/per-token rate limiting.
17. Confirm GitHub Actions secrets are configured: `NPM_TOKEN`, `DOCKERHUB_TOKEN` for Docker Hub user `tobilg`, and `HOMEBREW_TAP_TOKEN` with write access to `tobilg/homebrew-latlng`.
18. Run a local release dry run with `make act-release` when release workflow behavior changed.
19. Confirm release artifacts include OS binary archives, `openapi.json`, SHA256 checksum files, npm packages, the `tobilg/latlng:<tag>` Docker Hub image, and the `tobilg/homebrew-latlng` formula update.
20. Confirm the remaining open gap list still only contains deliberate non-release items, not regressions.
