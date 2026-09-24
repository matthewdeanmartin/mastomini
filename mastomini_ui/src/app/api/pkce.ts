// PKCE (RFC 7636) and the random values the sign-in flow needs.
//
// `crypto.getRandomValues` works on plain http; `crypto.subtle` does not,
// which is why the digest goes through sha256.ts (see there).

import { digestSha256 } from './sha256';

export function base64url(bytes: Uint8Array): string {
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** 32 random bytes, base64url: 43 characters, as RFC 7636 recommends. */
export function randomToken(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return base64url(bytes);
}

/** The S256 challenge for a verifier. */
export async function challengeFor(verifier: string): Promise<string> {
  return base64url(await digestSha256(new TextEncoder().encode(verifier)));
}
