import { LatLngClient } from "../src/index.js";

async function main(): Promise<void> {
  const client = new LatLngClient({
    leaderUrl: "http://leader:7421",
    readReplicas: ["http://follower-1:7421", "http://follower-2:7421"],
    readPreference: "followerPreferred",
    token: "dev-token",
  });

  const info = await client.server();
  console.log(info);
}

void main();
