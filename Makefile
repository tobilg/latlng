SHELL := /bin/sh

CARGO ?= cargo
NPM ?= npm
NPM_CACHE ?= /tmp/latlng-npm-cache
NODE ?= node
DOCKER ?= docker
COMPOSE ?= docker compose
ARTIFACT_SUFFIX ?= linux-x64
BINARY_EXT ?=
DIST_DIR ?= dist/$(ARTIFACT_SUFFIX)
OPENAPI_OUTPUT ?= dist/openapi.json
DOCKER_IMAGE ?= latlng-server:local
DOCKER_PREBUILT_IMAGE ?= latlng-server:local-prebuilt
LATLNG_SERVER_BINARY ?= target/release/latlng-server
LATLNG_SERVER_BENCHMARK_BINARY ?= target/release/latlng-server-benchmark
BENCHMARK_RESULTS_DIR ?= benchmark-results
BENCH_SERVER_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-memory.json
BENCH_SERVER_CAPNP_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-capnp.json
BENCH_SERVER_AOF_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-aof.json
BENCH_SERVER_TILE38_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-tile38.json
BENCH_FLAGS ?=
COMPARE_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-compare.json
COMPARE_CAPNP_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-compare-capnp.json
COMPARE_TILE38_OUTPUT ?= $(BENCHMARK_RESULTS_DIR)/bench-server-compare-tile38.json

SDK_DIR ?= packages/sdk
WASM_DIR ?= packages/wasm
EXAMPLE_WASM_DIR ?= packages/example-wasm

ACT ?= act
ACT_ARCH ?= linux/amd64
ACT_FLAGS ?= --container-architecture $(ACT_ARCH)
ACT_RELEASE_EVENT ?= .github/act/release-tag-push.json

.PHONY: help \
	fmt fmt-check clippy test check-wasm check build build-release build-server build-server-release build-cli build-cli-release build-benchmark build-server-benchmark package-release-binaries \
	verify-release-binaries openapi \
	bench-server bench-server-capnp bench-server-aof bench-server-tile38 bench-server-write-heavy bench-server-aof-tuning bench-server-query-heavy bench-server-geofence-heavy bench-server-compare bench-server-compare-capnp bench-server-compare-tile38 \
		sdk-install sdk-build sdk-docs sdk-typecheck sdk-test-unit sdk-test-integration sdk-test \
	wasm-install wasm-build wasm-typecheck wasm-test wasm-pack-dry-run \
	example-wasm-install example-wasm-build example-wasm-typecheck example-wasm-preview example-wasm-deploy \
	bump-version \
		docker-build docker-build-prebuilt docker-build-prebuilt-local docker-up docker-down docker-up-replication docker-down-replication \
	act-ci act-ci-rust act-ci-sdk act-ci-wasm act-ci-docker act-release act-release-binaries act-release-github act-release-container

help:
	@printf '%s\n' \
	'Rust:' \
	'  make fmt                  # cargo fmt --all' \
	'  make fmt-check            # cargo fmt --all --check' \
	'  make clippy               # cargo clippy --workspace --all-targets' \
	'  make test                 # cargo test --workspace' \
	'  make check-wasm           # portable wasm cargo check' \
	'  make check                # clippy + test + wasm check' \
	'  make build                # cargo build --workspace' \
	'  make build-release        # cargo build --release --workspace' \
	'  make build-server         # cargo build -p latlng-server' \
	'  make build-server-release # cargo build --release -p latlng-server' \
	'  make build-cli            # cargo build -p latlng-cli' \
	'  make build-cli-release    # cargo build --release -p latlng-cli' \
	'  make build-benchmark      # cargo build --release -p latlng-benchmark' \
	'  make build-server-benchmark # cargo build --release -p latlng-server-benchmark' \
	'  make openapi              # generate dist/openapi.json from latlng-server --print-openapi' \
	'  make bench-server         # run manual localhost server benchmarks in memory mode' \
	'  make bench-server-capnp   # run latlng benchmarks over Capn Proto' \
	'  make bench-server-aof     # run manual localhost server benchmarks in aof mode' \
	'  make bench-server-tile38  # run Tile38 localhost server benchmarks' \
	'  make bench-server-write-heavy # run write-heavy server benchmarks in memory mode' \
	'  make bench-server-aof-tuning  # run write-heavy AOF tuning benchmarks' \
	'  make bench-server-query-heavy # run query-heavy server benchmarks in memory mode' \
	'  make bench-server-geofence-heavy # run geofence/channel-heavy write benchmarks' \
	'  make bench-server-compare OLD=old.json NEW=new.json [COMPARE_OUTPUT=cmp.json]' \
	'  make bench-server-compare-capnp OLD=http.json NEW=capnp.json [COMPARE_CAPNP_OUTPUT=cmp.json]' \
	'  make bench-server-compare-tile38 OLD=latlng.json NEW=tile38.json [COMPARE_TILE38_OUTPUT=cmp.json]' \
	'  make package-release-binaries ARTIFACT_SUFFIX=linux-x64 [BINARY_EXT=.exe]' \
	'  make verify-release-binaries ARTIFACT_SUFFIX=linux-x64 [BINARY_EXT=.exe]' \
	'' \
		'TypeScript SDK:' \
		'  make sdk-install          # npm ci in packages/sdk' \
		'  make sdk-build            # npm run build' \
		'  make sdk-docs             # npm run docs:api' \
		'  make sdk-typecheck        # npm run typecheck' \
	'  make sdk-test-unit        # npm run test:unit' \
	'  make sdk-test-integration # npm run test:integration' \
	'  make sdk-test             # npm run test' \
	'' \
	'Browser wasm package:' \
	'  make wasm-install         # npm ci in packages/wasm' \
	'  make wasm-build           # npm run build in packages/wasm' \
	'  make wasm-typecheck       # npm run typecheck in packages/wasm' \
	'  make wasm-test            # npm run test in packages/wasm' \
	'  make wasm-pack-dry-run    # npm pack --dry-run in packages/wasm' \
	'' \
	'Browser wasm example:' \
	'  make example-wasm-install # npm ci in packages/example-wasm' \
	'  make example-wasm-build   # npm run build in packages/example-wasm' \
	'  make example-wasm-typecheck # npm run typecheck in packages/example-wasm' \
	'  make example-wasm-preview # npm run preview in packages/example-wasm' \
	'  make example-wasm-deploy  # npm run deploy in packages/example-wasm' \
	'' \
	'Versioning:' \
	'  make bump-version V=x.y.z # sync Cargo/npm package versions and lockfiles' \
	'' \
		'Docker:' \
		'  make docker-build         # docker build -t latlng-server:local .' \
		'  make docker-build-prebuilt# build runtime-prebuilt image from LATLNG_SERVER_BINARY' \
		'  make docker-build-prebuilt-local # build server, then build runtime-prebuilt image' \
	'  make docker-up            # docker compose up --build -d' \
	'  make docker-down          # docker compose down' \
	'  make docker-up-replication    # docker compose -f docker-compose.replication.yml up -d' \
	'  make docker-down-replication  # docker compose -f docker-compose.replication.yml down' \
	'' \
	'GitHub Actions via act:' \
	'  make act-ci' \
	'  make act-ci-rust' \
	'  make act-ci-sdk' \
	'  make act-ci-wasm' \
	'  make act-ci-docker' \
	'  make act-release' \
	'  make act-release-binaries' \
	'  make act-release-github' \
	'  make act-release-container'

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test:
	$(CARGO) test --workspace

check-wasm:
	$(CARGO) check --target wasm32-unknown-unknown \
		-p latlng-platform \
		-p latlng-geo \
		-p latlng-index \
		-p latlng-storage \
		-p latlng-core \
		-p latlng-geofence \
		-p latlng-storage-memory
	$(CARGO) check --target wasm32-unknown-unknown -p latlng-core --features wasm-bindings
	$(CARGO) check --target wasm32-unknown-unknown -p latlng-core --features wasm-browser-bindings

check: clippy test check-wasm

build:
	$(CARGO) build --workspace

build-release:
	$(CARGO) build --release --workspace

build-server:
	$(CARGO) build -p latlng-server

build-server-release:
	$(CARGO) build --release -p latlng-server

build-cli:
	$(CARGO) build -p latlng-cli

build-cli-release:
	$(CARGO) build --release -p latlng-cli

build-benchmark:
	$(CARGO) build --release -p latlng-benchmark

build-server-benchmark:
	$(CARGO) build --release -p latlng-server-benchmark

bench-server: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--profile memory \
		--scenario all \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCH_SERVER_OUTPUT) \
		$(BENCH_FLAGS)

bench-server-capnp: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--profile memory \
		--scenario all \
		--latlng-transport capnp \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCH_SERVER_CAPNP_OUTPUT) \
		$(BENCH_FLAGS)

bench-server-aof: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--profile aof \
		--scenario all \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCH_SERVER_AOF_OUTPUT) \
		$(BENCH_FLAGS)

bench-server-tile38: build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--engine tile38 \
		--profile memory \
		--scenario all \
		--output $(BENCH_SERVER_TILE38_OUTPUT) \
		$(BENCH_FLAGS)

bench-server-write-heavy: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--preset write-heavy \
		--profile memory \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCHMARK_RESULTS_DIR)/bench-server-write-heavy.json \
		$(BENCH_FLAGS)

bench-server-aof-tuning: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--preset aof-tuning \
		--profile aof \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCHMARK_RESULTS_DIR)/bench-server-aof-tuning.json \
		$(BENCH_FLAGS)

bench-server-query-heavy: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--preset query-heavy \
		--profile memory \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCHMARK_RESULTS_DIR)/bench-server-query-heavy.json \
		$(BENCH_FLAGS)

bench-server-geofence-heavy: build-server-release build-server-benchmark
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) run \
		--preset geofence-heavy \
		--profile memory \
		--server-bin $(LATLNG_SERVER_BINARY) \
		--output $(BENCHMARK_RESULTS_DIR)/bench-server-geofence-heavy.json \
		$(BENCH_FLAGS)

bench-server-compare: build-server-benchmark
ifndef OLD
	$(error Usage: make bench-server-compare OLD=old.json NEW=new.json [COMPARE_OUTPUT=cmp.json])
endif
ifndef NEW
	$(error Usage: make bench-server-compare OLD=old.json NEW=new.json [COMPARE_OUTPUT=cmp.json])
endif
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) compare $(OLD) $(NEW) $(if $(COMPARE_OUTPUT),--output $(COMPARE_OUTPUT),)

bench-server-compare-capnp: build-server-benchmark
ifndef OLD
	$(error Usage: make bench-server-compare-capnp OLD=http.json NEW=capnp.json [COMPARE_CAPNP_OUTPUT=cmp.json])
endif
ifndef NEW
	$(error Usage: make bench-server-compare-capnp OLD=http.json NEW=capnp.json [COMPARE_CAPNP_OUTPUT=cmp.json])
endif
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) compare $(OLD) $(NEW) $(if $(COMPARE_CAPNP_OUTPUT),--output $(COMPARE_CAPNP_OUTPUT),)

bench-server-compare-tile38: build-server-benchmark
ifndef OLD
	$(error Usage: make bench-server-compare-tile38 OLD=latlng.json NEW=tile38.json [COMPARE_TILE38_OUTPUT=cmp.json])
endif
ifndef NEW
	$(error Usage: make bench-server-compare-tile38 OLD=latlng.json NEW=tile38.json [COMPARE_TILE38_OUTPUT=cmp.json])
endif
	mkdir -p $(BENCHMARK_RESULTS_DIR)
	$(LATLNG_SERVER_BENCHMARK_BINARY) compare $(OLD) $(NEW) $(if $(COMPARE_TILE38_OUTPUT),--output $(COMPARE_TILE38_OUTPUT),)

package-release-binaries:
	mkdir -p "$(DIST_DIR)/bin"
	cp "target/release/latlng-server$(BINARY_EXT)" "$(DIST_DIR)/bin/latlng-server$(BINARY_EXT)"
	cp "target/release/latlng-cli$(BINARY_EXT)" "$(DIST_DIR)/bin/latlng-cli$(BINARY_EXT)"
	tar -czf "dist/latlng-$(ARTIFACT_SUFFIX).tar.gz" -C "$(DIST_DIR)/bin" .

verify-release-binaries:
	archive="dist/latlng-$(ARTIFACT_SUFFIX).tar.gz"; \
	test -f "$$archive"; \
	entries="$$(tar -tzf "$$archive" | sed 's#^\./##' | grep -v '^$$' | grep -v '/$$' | sort | tr '\n' ' ')"; \
	expected="latlng-cli$(BINARY_EXT) latlng-server$(BINARY_EXT) "; \
	test "$$entries" = "$$expected"

openapi:
	mkdir -p "$$(dirname "$(OPENAPI_OUTPUT)")"
	$(CARGO) run -p latlng-server -- --print-openapi > "$(OPENAPI_OUTPUT)"
	$(NODE) -e 'const fs=require("node:fs"); JSON.parse(fs.readFileSync(process.argv[1], "utf8"));' "$(OPENAPI_OUTPUT)"

sdk-install:
	cd $(SDK_DIR) && $(NPM) ci

sdk-build:
	cd $(SDK_DIR) && $(NPM) run build

sdk-docs:
	cd $(SDK_DIR) && $(NPM) run docs:api

sdk-typecheck:
	cd $(SDK_DIR) && $(NPM) run typecheck

sdk-test-unit:
	cd $(SDK_DIR) && $(NPM) run test:unit

sdk-test-integration:
	cd $(SDK_DIR) && $(NPM) run test:integration

sdk-test:
	cd $(SDK_DIR) && $(NPM) run test

wasm-install:
	cd $(WASM_DIR) && $(NPM) ci

wasm-build:
	cd $(WASM_DIR) && $(NPM) run build

wasm-typecheck:
	cd $(WASM_DIR) && $(NPM) run typecheck

wasm-test:
	cd $(WASM_DIR) && $(NPM) run test

wasm-pack-dry-run:
	cd $(WASM_DIR) && $(NPM) --cache $(NPM_CACHE) pack --dry-run

example-wasm-install:
	cd $(EXAMPLE_WASM_DIR) && $(NPM) ci

example-wasm-build:
	cd $(EXAMPLE_WASM_DIR) && $(NPM) run build

example-wasm-typecheck:
	cd $(EXAMPLE_WASM_DIR) && $(NPM) run typecheck

example-wasm-preview:
	cd $(EXAMPLE_WASM_DIR) && $(NPM) run preview

example-wasm-deploy:
	cd $(EXAMPLE_WASM_DIR) && $(NPM) run deploy

bump-version:
ifndef V
	$(error Usage: make bump-version V=x.y.z)
endif
	$(NODE) scripts/bump-version.mjs "$(V)"

docker-build:
	$(DOCKER) build -t $(DOCKER_IMAGE) .

docker-build-prebuilt:
	$(DOCKER) build \
		--target runtime-prebuilt \
		--build-arg LATLNG_SERVER_BINARY=$(LATLNG_SERVER_BINARY) \
		-t $(DOCKER_PREBUILT_IMAGE) \
		.

docker-build-prebuilt-local: build-server-release docker-build-prebuilt

docker-up:
	$(COMPOSE) up --build -d

docker-down:
	$(COMPOSE) down

docker-up-replication:
	$(COMPOSE) -f docker-compose.replication.yml up -d

docker-down-replication:
	$(COMPOSE) -f docker-compose.replication.yml down

act-ci:
	$(ACT) pull_request -W .github/workflows/ci.yml $(ACT_FLAGS)

act-ci-rust:
	$(ACT) pull_request -W .github/workflows/ci.yml -j rust $(ACT_FLAGS)

act-ci-sdk:
	$(ACT) pull_request -W .github/workflows/ci.yml -j typescript-sdk $(ACT_FLAGS)

act-ci-wasm:
	$(ACT) pull_request -W .github/workflows/ci.yml -j typescript-wasm $(ACT_FLAGS)

act-ci-docker:
	$(ACT) pull_request -W .github/workflows/ci.yml -j docker $(ACT_FLAGS)

act-release:
	$(ACT) push -W .github/workflows/release.yml -e $(ACT_RELEASE_EVENT) $(ACT_FLAGS)

act-release-binaries:
	$(ACT) push -W .github/workflows/release.yml -e $(ACT_RELEASE_EVENT) -j binaries $(ACT_FLAGS)

act-release-github:
	$(ACT) push -W .github/workflows/release.yml -e $(ACT_RELEASE_EVENT) -j github-release $(ACT_FLAGS)

act-release-container:
	$(ACT) push -W .github/workflows/release.yml -e $(ACT_RELEASE_EVENT) -j container-release $(ACT_FLAGS)
