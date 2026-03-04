import { makeHash } from "../common.ts";

// Resolve a friendly service name from an ID. This is a simple deterministic slug:
//   svc-<first 8 hex of sha256(id)>
export function getServiceNameById(id: string): string {
	const hash = makeHash(id, "sha256");
	return `svc-${hash.slice(0, 8)}`;
}

export const getEcho = () => [
	"echo Hello from helper.ts!"
];

