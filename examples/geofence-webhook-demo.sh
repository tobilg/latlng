#!/usr/bin/env bash

set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:7421}"
WEBHOOK_URL="${WEBHOOK_URL:-https://webhook.site/26a40558-e8cf-4cfd-8ffa-cfa372008c64}"
COLLECTION="${COLLECTION:-fleet}"
OBJECT_ID="${OBJECT_ID:-truck-1}"
HOOK_NAME="${HOOK_NAME:-fleet-demo-hook}"

AUTH_ARGS=()
if [[ -n "${LATLNG_TOKEN:-}" ]]; then
  AUTH_ARGS=(-H "Authorization: Bearer ${LATLNG_TOKEN}")
fi

curl_json() {
  local method="$1"
  local path="$2"
  local body="${3:-}"

  echo
  echo ">>> ${method} ${BASE_URL}${path}"
  if [[ -n "${body}" ]]; then
    curl -sS -X "${method}" \
      "${AUTH_ARGS[@]}" \
      -H 'content-type: application/json' \
      "${BASE_URL}${path}" \
      -d "${body}"
  else
    curl -sS -X "${method}" \
      "${AUTH_ARGS[@]}" \
      "${BASE_URL}${path}"
  fi
  echo
}

seed_body=$(cat <<JSON
{
  "object": {
    "Point": {
      "lat": 52.5000,
      "lon": 13.3500,
      "z": null
    }
  }
}
JSON
)

hook_body=$(cat <<JSON
{
  "name": "${HOOK_NAME}",
  "endpoint": "${WEBHOOK_URL}",
  "def": {
    "collection": "${COLLECTION}",
    "query": {
      "Nearby": {
        "lat": 52.5200,
        "lon": 13.4050,
        "meters": 250.0,
        "options": {}
      }
    },
    "detect": ["Enter", "Exit"],
    "commands": ["Set"]
  }
}
JSON
)

update_body=$(cat <<JSON
{
  "object": {
    "Point": {
      "lat": 52.5200,
      "lon": 13.4050,
      "z": null
    }
  }
}
JSON
)

exit_body=$(cat <<JSON
{
  "object": {
    "Point": {
      "lat": 55.5200,
      "lon": 10.4050,
      "z": null
    }
  }
}
JSON
)

echo "Seeding ${COLLECTION}/${OBJECT_ID} outside the geofence to create the collection."
curl_json POST "/collections/${COLLECTION}/objects/${OBJECT_ID}" "${seed_body}"

echo "Registering a webhook geofence."
curl_json POST "/hooks" "${hook_body}"

echo "Listing the registered hooks."
curl_json GET "/hooks"

echo "Moving ${COLLECTION}/${OBJECT_ID} into the geofence."
curl_json POST "/collections/${COLLECTION}/objects/${OBJECT_ID}" "${update_body}"

echo "Moving ${COLLECTION}/${OBJECT_ID} out of the geofence."
curl_json POST "/collections/${COLLECTION}/objects/${OBJECT_ID}" "${exit_body}"

echo "Current server metrics."
curl_json GET "/metrics"

cat <<EOF

If the server can reach the public internet, webhook.site should receive two events with:
- command: "Set"
- detect: "Enter"
- collection: "${COLLECTION}"
- id: "${OBJECT_ID}"
- hook: "${HOOK_NAME}"
- command: "Set"
- detect: "Exit"
- collection: "${COLLECTION}"
- id: "${OBJECT_ID}"
- hook: "${HOOK_NAME}"

Webhook inbox:
${WEBHOOK_URL}
EOF
