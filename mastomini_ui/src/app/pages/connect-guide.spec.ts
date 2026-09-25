import { GUIDES, detectDevice, works } from './connect-guide';

describe('connect guide', () => {
  it('detects devices from user agents', () => {
    expect(detectDevice('Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)')).toBe('ios');
    expect(detectDevice('Mozilla/5.0 (Linux; Android 14; Pixel 8)')).toBe('android');
    expect(detectDevice('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)')).toBe('mac');
    expect(detectDevice('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe('windows');
    expect(detectDevice('Mozilla/5.0 (X11; Linux x86_64)')).toBe('linux');
  });

  it('says only plain-HTTP clients work without HTTPS', () => {
    const all = GUIDES.flatMap((g) => g.clients);
    const now = all.filter((c) => works(c, false)).map((c) => c.name);
    expect(now).toEqual(['Mastodon.py and scripts']);
  });

  it('with the household certificate, all but real-domain-only clients work', () => {
    const all = GUIDES.flatMap((g) => g.clients);
    const blocked = all.filter((c) => !works(c, true)).map((c) => c.name);
    expect(blocked.sort()).toEqual(['Moshidon', 'Mastodon (official)', 'Tusky'].sort());
    expect(all.filter((c) => !works(c, true)).every((c) => c.needs.includes('https-domain'))).toBe(
      true,
    );
  });

  it('has a guide for every device kind', () => {
    const kinds = new Set(GUIDES.map((g) => g.kind));
    expect([...kinds].sort()).toEqual(['android', 'ios', 'linux', 'mac', 'windows']);
  });
});
