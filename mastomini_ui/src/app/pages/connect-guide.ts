// Which clients to suggest on which device (spec/06 "Client setup guide").
//
// Data, not code, so the tested-client matrix from spec/07 can update it
// without touching the page. `tested` means "signed in against a mastomini
// server by the project"; nothing is marked tested on a guess.

export type DeviceKind = 'ios' | 'android' | 'mac' | 'windows' | 'linux';

/** What the client needs from the server's connection (spec/05). */
export type Needs =
  /** Works over plain HTTP. */
  | 'http'
  /** HTTPS, trusting the household certificate (Model A). */
  | 'https-ca'
  /** HTTPS with a real domain and a public certificate (Model B). */
  | 'https-domain';

export interface Client {
  name: string;
  needs: Needs[];
  tested: boolean;
  note?: string;
}

export interface DeviceGuide {
  kind: DeviceKind;
  label: string;
  clients: Client[];
  notes: string[];
}

export const GUIDES: DeviceGuide[] = [
  {
    kind: 'ios',
    label: 'iPhone / iPad',
    clients: [
      { name: 'Ivory', needs: ['https-ca', 'https-domain'], tested: false },
      { name: 'Ice Cubes', needs: ['https-ca', 'https-domain'], tested: false },
      { name: 'Mona', needs: ['https-ca'], tested: false },
      { name: 'Mastodon (official)', needs: ['https-ca'], tested: false },
    ],
    notes: [
      'iPhone apps require HTTPS. Once the owner turns it on, install the household certificate from the Trust page and turn on full trust for it.',
    ],
  },
  {
    kind: 'android',
    label: 'Android',
    clients: [
      { name: 'Tusky', needs: ['https-domain'], tested: false },
      { name: 'Moshidon', needs: ['https-domain'], tested: false },
      { name: 'Mastodon (official)', needs: ['https-domain'], tested: false },
    ],
    notes: [
      'Most Android apps ignore certificates you install yourself, so they need the server on a real domain name with a public certificate.',
      "Some routers block a public name that points at a home address (\"DNS rebinding protection\"); the owner may need to allow the name in the router or Pi-hole.",
    ],
  },
  {
    kind: 'mac',
    label: 'Mac',
    clients: [
      { name: 'Ivory', needs: ['https-ca', 'https-domain'], tested: false },
      { name: 'Ice Cubes', needs: ['https-ca', 'https-domain'], tested: false },
      { name: 'Mona', needs: ['https-ca'], tested: false },
    ],
    notes: ['Trust the household certificate in Keychain Access, or use a real domain.'],
  },
  {
    kind: 'windows',
    label: 'Windows',
    clients: [
      { name: 'Whalebird', needs: ['https-ca'], tested: false },
      { name: 'Phanpy (web)', needs: ['https-ca', 'https-domain'], tested: false, note: 'Only if the browser trusts the certificate.' },
    ],
    notes: [
      'Web clients that run on their own servers (such as Elk) cannot reach a server on your home network.',
    ],
  },
  {
    kind: 'linux',
    label: 'Linux',
    clients: [
      { name: 'Tuba', needs: ['https-ca'], tested: false },
      { name: 'Whalebird', needs: ['https-ca'], tested: false },
      { name: 'Mastodon.py and scripts', needs: ['http'], tested: true },
    ],
    notes: [],
  },
];

/** The device this browser is on, from its User-Agent. */
export function detectDevice(userAgent: string): DeviceKind {
  const ua = userAgent.toLowerCase();
  if (/iphone|ipad|ipod/.test(ua)) return 'ios';
  if (ua.includes('android')) return 'android';
  // iPadOS asks for desktop sites and says "Macintosh"; touch gives it away.
  if (ua.includes('macintosh')) return 'mac';
  if (ua.includes('windows')) return 'windows';
  return 'linux';
}

/** Can this client connect with what the server offers now? */
export function works(client: Client, https: boolean): boolean {
  return client.needs.includes('http') || (https && client.needs.some((n) => n !== 'http'));
}

export const NEEDS_TEXT: Record<Needs, string> = {
  http: 'works over plain HTTP',
  'https-ca': 'needs HTTPS with the household certificate',
  'https-domain': 'needs HTTPS on a real domain name',
};
