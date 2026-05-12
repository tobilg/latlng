import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";

const args = parseArgs(process.argv.slice(2));
const version = normalizeVersion(requireArg(args, "version"));
const sha256 = normalizeSha256(requireArg(args, "sha256"));
const artifactUrl =
  args["artifact-url"] ??
  `https://github.com/tobilg/latlng/releases/download/v${version}/latlng-macos-arm64.tar.gz`;
const output = args.output ? resolve(args.output) : null;
const formula = renderFormula({ artifactUrl, sha256, version });

if (output) {
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, formula);
} else {
  process.stdout.write(formula);
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) {
      throw new Error(`Unexpected argument: ${arg}`);
    }
    const key = arg.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`Missing value for --${key}`);
    }
    parsed[key] = value;
    index += 1;
  }
  return parsed;
}

function requireArg(args, key) {
  const value = args[key];
  if (!value) {
    throw new Error(`Missing required --${key}`);
  }
  return value;
}

function normalizeVersion(raw) {
  const version = raw.trim().replace(/^v/i, "");
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid version: ${raw}`);
  }
  return version;
}

function normalizeSha256(raw) {
  const sha256 = raw.trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(sha256)) {
    throw new Error(`Invalid SHA256: ${raw}`);
  }
  return sha256;
}

function renderFormula({ artifactUrl, sha256, version }) {
  return `class Latlng < Formula
  desc "Geospatial object server and command-line tools"
  homepage "https://github.com/tobilg/latlng"
  url "${artifactUrl}"
  sha256 "${sha256}"
  version "${version}"
  license "MIT"

  depends_on :macos
  depends_on arch: :arm64

  def default_config
    <<~TOML
      listen_addr = "127.0.0.1:7421"
      capnp_enabled = false
      capnp_listen_addr = "127.0.0.1:7422"
      server_id = "latlng-homebrew-local"
      webhook_queue_path = "#{var}/latlng/webhook-queue.sqlite"
      log_destination = "file"
      log_file_path = "#{var}/log/latlng/latlng-server.log"

      [storage]
      type = "aof"
      path = "#{var}/latlng/appendonly.aof"
    TOML
  end

  def install
    bin.install "latlng-server"
    bin.install "latlng-cli"

    (etc/"latlng").mkpath
    (var/"latlng").mkpath
    (var/"log/latlng").mkpath

    config = etc/"latlng/latlng.toml"
    config.write default_config unless config.exist?
  end

  service do
    run [opt_bin/"latlng-server", "--config", etc/"latlng/latlng.toml"]
    keep_alive true
    working_dir var/"latlng"
    log_path var/"log/latlng/latlng-server.log"
    error_log_path var/"log/latlng/latlng-server.log"
  end

  def caveats
    <<~EOS
      Default service config:
        #{etc}/latlng/latlng.toml

      Default data directory:
        #{var}/latlng

      Default log file:
        #{var}/log/latlng/latlng-server.log

      The service listens on 127.0.0.1:7421 and uses AOF persistence by default.

      Start latlng as a background service:
        brew services start latlng

      Run manually:
        latlng-server --config #{etc}/latlng/latlng.toml
    EOS
  end

  test do
    assert_match "latlng-server #{version}", shell_output("#{bin}/latlng-server --version")
    assert_match "latlng-cli", shell_output("#{bin}/latlng-cli --help 2>&1")

    (testpath/"latlng.toml").write <<~TOML
      listen_addr = "127.0.0.1:7421"
      webhook_queue_path = "#{testpath}/webhook-queue.sqlite"

      [storage]
      type = "aof"
      path = "#{testpath}/appendonly.aof"
    TOML

    assert_match "\\"ok\\": true", shell_output("#{bin}/latlng-server --config #{testpath}/latlng.toml --check-config")
  end
end
`;
}
