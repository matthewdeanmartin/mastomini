// What the server sends. Only the fields this app reads.

export type Role = 'owner' | 'admin' | 'member';

/** A Mastodon `Account` (`CredentialAccount` for verify_credentials). */
export interface Account {
  id: string;
  username: string;
  display_name: string;
  /** Present on `CredentialAccount`: Mastodon's role object. */
  role?: { id: string; name: string };
}

/** The role from a credential account: mastomini's role ids are fixed. */
export function roleOf(account: Account): Role {
  switch (account.role?.id) {
    case '0':
      return 'owner';
    case '3':
      return 'admin';
    default:
      return 'member';
  }
}

/** A row of `GET /api/mastomini/v1/admin/members`. */
export interface Member {
  id: string;
  username: string;
  display_name: string;
  role: Role;
  disabled: boolean;
  silenced: boolean;
  suspended: boolean;
}

/** An invite or reset code; `code` and `url` only when just created. */
export interface Code {
  id: string;
  kind: 'invite' | 'reset';
  created_at: string;
  expires_at: string;
  account_id?: string;
  username?: string | null;
  code?: string;
  url?: string;
}

/** A row of `GET /api/mastomini/v1/me/devices`. */
export interface Device {
  id: string;
  app: { name: string | null; website: string | null };
  scopes: string[];
  created_at: string;
  last_used_at: string | null;
  current: boolean;
}

/** `GET/PUT /api/mastomini/v1/admin/server`. */
export interface ServerSettings {
  title: string;
  description: string;
  rules: string[];
  terms: string;
  terms_customized: boolean;
  terms_effective_date: string;
}

/** `GET /api/mastomini/v1/status`. */
export interface Status {
  provisioned: boolean;
  title: string;
  version: string;
  host: string;
  available: boolean;
  accounts: number;
  statuses: number;
  evictions: number;
  writes_this_hour: number;
  store_used: number | null;
  /** `synced`, `manual` (set from a browser) or `unset`. */
  clock: 'synced' | 'manual' | 'unset';
  /** `http` (no certificate) or `easy` (HTTPS and HTTP); later `secure`. */
  mode: string;
  /** The server has HTTPS (the household certificate). */
  https: boolean;
  /** This request came over HTTPS. */
  secure: boolean;
  /** Where to use HTTPS, e.g. `https://mastomini.local`. */
  https_url: string | null;
  /** The household CA's SHA-256 fingerprint, to compare on each device. */
  ca_fingerprint: string | null;
  household_app: boolean;
}

/** `[in use, limit]`. */
export type Usage = [number, number];

/** `GET /api/mastomini/v1/diag`. */
export interface Diag {
  version: string;
  household_app: boolean;
  platform: {
    target: string;
    uptime_ms: number;
    heap_internal_free: number | null;
    heap_internal_min_free: number | null;
    heap_internal_largest_block: number | null;
    psram_free: number | null;
    psram_min_free: number | null;
    reset_reason: string | null;
    wifi_rssi: number | null;
  };
  clock: { source: Status['clock']; now: string | null };
  store: {
    available: boolean;
    problem: string | null;
    entries_used: number | null;
    entries_total: number | null;
    used: number | null;
    low_watermark: number;
    evictions_since_boot: number;
    repairs_at_boot: number;
    oldest_post_at: string | null;
  };
  records: Record<string, Usage>;
  governor: {
    writes_this_hour: number;
    limit_per_hour: number;
    limit_per_member_hour: number;
    window_started_at: string | null;
  };
}

/** `GET /api/mastomini/v1/admin/security`. */
export interface Security {
  transport: {
    mode: string;
    https: boolean;
    secure_mode_available: boolean;
    this_connection: 'http' | 'https';
  };
  /** The board's HTTPS certificate; null without HTTPS. */
  certificate: { names: string[]; not_after: string; https_url: string } | null;
  household_ca: { name: string; fingerprint_sha256: string; name_constrained: boolean } | null;
  passwords: { min_length: number; rounds: number; lockout_failures: number; lockout_minutes: number };
  apps: Usage;
  tokens: Usage;
  tokens_per_member: number;
  members: { username: string; role: Role; devices: number; message_key: boolean }[];
}
