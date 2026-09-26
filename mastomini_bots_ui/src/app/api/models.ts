// Shapes of the bots board's /api/v1 answers (mastomini_bots/src/api.rs).

export interface Build {
  name: string;
  version: string;
  commit: string | null;
  dirty: boolean;
  built_at: string | null;
}

export interface Status {
  setup_needed: boolean;
  signed_in: boolean;
  /** This connection is HTTPS. */
  secure: boolean;
  /** The certificate this build serves, if it serves HTTPS. */
  https: { names: string[]; not_after: string; ca_name: string } | null;
  /** Wall-clock time, once internet time is set. */
  clock: string | null;
  build: Build;
}

export interface RunRecord {
  slot_at: string;
  started_at: string;
  finished_at: string;
  manual: boolean;
  ok: boolean;
  summary: string;
}

/** One of a bot's settings, as its code declared it (mastomini_bots/src/settings.rs). */
export interface SettingField {
  key: string;
  label: string;
  help: string;
  kind: 'text' | 'long_text' | 'number' | 'choice' | 'time' | 'zone' | 'toggle' | 'secret';
  default: string;
  optional: boolean;
  /** long_text: the {{variables}} it may use. */
  placeholders?: string[];
  /** number */
  min?: number;
  max?: number;
  /** choice, zone: [value, label] */
  options?: [string, string][];
  /** Everything but secrets. */
  value?: string;
  /** secret: whether one is saved. */
  set?: boolean;
}

export interface Bot {
  id: string;
  name: string;
  description: string;
  schedule: string;
  grace_minutes: number;
  enabled: boolean;
  instance: string;
  /** An API key is saved. The key itself never comes back. */
  key_set: boolean;
  running: boolean;
  queued: boolean;
  next_run_at: string | null;
  retrying: boolean;
  last_run: RunRecord | null;
  last_check: RunRecord | null;
  runs: number;
  failures: number;
  /** Needs the device's OpenRouter key. */
  uses_llm: boolean;
  settings: SettingField[];
}

/** `GET /api/v1/integrations/openrouter`. */
export interface OpenRouterStatus {
  key_set: boolean;
  /** Empty: the default. */
  model: string;
  default_model: string;
  used_by: string[];
}

/** Omitted fields stay as they are; an empty `token` removes the key. */
export interface BotUpdate {
  enabled?: boolean;
  instance?: string;
  token?: string;
  /** Changed settings only, by key. */
  settings?: Record<string, string>;
}

export interface ActivityEvent {
  seq: number;
  at: string | null;
  bot: string | null;
  level: 'info' | 'ok' | 'error';
  text: string;
}

export interface Diag {
  platform: {
    target: string;
    uptime_ms: number;
    heap_internal_free: number | null;
    heap_internal_min_free: number | null;
    psram_free: number | null;
    wifi_rssi: number | null;
    reset_reason: string | null;
    ip: string | null;
  };
  clock: string | null;
  build: Build;
}
