import { LatLngClient, point } from "../src/index.js";

async function main(): Promise<void> {
  const client = new LatLngClient({
    leaderUrl: "http://127.0.0.1:7421",
    token: "dev-token",
  });

  await client.setObject("fleet", "truck-1", point(52.52, 13.405));
  const object = await client.get("fleet", "truck-1");
  const nearby = await client.nearby("fleet", {
    lat: 52.52,
    lon: 13.405,
    meters: 500,
  });

  console.log(object);
  console.log(nearby);
}

void main();
