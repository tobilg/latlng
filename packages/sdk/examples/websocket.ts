import { LatLngClient } from "../src/index.js";

async function main(): Promise<void> {
  const client = new LatLngClient({
    leaderUrl: "http://127.0.0.1:7421",
    token: "dev-token",
  });

  const socket = await client.connectWebSocket();
  const subscription = await socket.psubscribe(["fleet*"]);

  subscription.on("event", (event) => {
    console.log(event.detect, event.id);
  });
}

void main();
