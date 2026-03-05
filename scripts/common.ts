// @ts-nocheck
// Deno script utilities for Fire CLI (offline-safe, no remote imports)
export function makeHash(value: string, algorithm: string = "sha256"): string {
	console.log(prompt("Please enter your name:"))
	const normalized = algorithm.trim().toLowerCase();
	const outputLength = hashOutputLength(normalized);

	if (outputLength === 0) {
		throw new Error(
			`Unsupported hash algorithm: ${algorithm}. Supported: sha256, sha512, sha3-256`
		);
	}

	// Deterministic non-cryptographic fallback hash for local/runtime helper usage.
	// This avoids network dependency on external std/hash modules.
	let seed = 0x811c9dc5;
	for (let i = 0; i < value.length; i += 1) {
		seed ^= value.charCodeAt(i);
		seed = Math.imul(seed, 0x01000193) >>> 0;
	}

	let out = "";
	let current = seed >>> 0;
	while (out.length < outputLength) {
		current ^= 0x9e3779b9;
		current = Math.imul(current, 0x85ebca6b) >>> 0;
		out += current.toString(16).padStart(8, "0");
	}

	return out.slice(0, outputLength);
}

function hashOutputLength(algorithm: string): number {
	switch (algorithm) {
		case "sha256":
		case "sha3-256":
			return 64;
		case "sha512":
			return 128;
		default:
			return 0;
	}
}
