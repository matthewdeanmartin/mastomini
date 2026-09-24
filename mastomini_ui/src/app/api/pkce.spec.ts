import { base64url, challengeFor, randomToken } from './pkce';

describe('pkce', () => {
  it('matches the RFC 7636 appendix B example', async () => {
    expect(await challengeFor('dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk')).toBe(
      'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
    );
  });

  it('encodes without padding or url-unsafe characters', () => {
    expect(base64url(new Uint8Array([0xfb, 0xff]))).toBe('-_8');
  });

  it('makes 43-character verifiers', () => {
    const token = randomToken();
    expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(randomToken()).not.toBe(token);
  });
});
