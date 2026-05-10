import type { ReadPreference } from "../types/requests.js";

export interface ReplicaCandidate {
  url: string;
  eligible: boolean;
}

export function chooseReplicaOrder(
  replicas: ReplicaCandidate[],
  preference: ReadPreference,
  roundRobinCursor: number,
): string[] {
  const eligible = replicas.filter((replica) => replica.eligible).map((replica) => replica.url);
  if (eligible.length === 0) {
    return [];
  }

  if (preference === "roundRobinFollowers") {
    const start = roundRobinCursor % eligible.length;
    return [...eligible.slice(start), ...eligible.slice(0, start)];
  }

  return eligible;
}
