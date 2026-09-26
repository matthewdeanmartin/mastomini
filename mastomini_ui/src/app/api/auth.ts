// Signing in: the same OAuth code flow (with PKCE) that every Mastodon app
// uses, through the server's own sign-in page. The app has no password form of
// its own, so a bug in OAuth shows up here first (spec/06).
//
// The app registers itself with `POST /api/v1/apps` the first time a browser
// signs in, with this page as the redirect URI. Registering per browser rather
// than once at provisioning means the redirect URI always matches the address
// the person actually typed (an IP, `mastomini.local`, or the dev server).

import { Injectable, InjectionToken, computed, inject, signal } from '@angular/core';

import { Api, ApiError } from './api';
import { Account, Role, roleOf } from './models';
import { challengeFor, randomToken } from './pkce';

const APP_KEY = 'mastomini.app';
const PENDING_KEY = 'mastomini.pending';
const SCOPES = 'read write';
const ADMIN_SCOPES = `${SCOPES} admin:read admin:write`;
const GRANT_KEY = 'mastomini.grant';
export const OAUTH_NAVIGATE = new InjectionToken<(url: string) => void>('OAuth navigation', {
  providedIn: 'root',
  factory: () => (url) => location.assign(url),
});

interface Registration {
  client_id: string;
  client_secret: string;
  redirect_uri: string;
  scopes: string;
}

interface Pending {
  state: string;
  verifier: string;
  /** The route to return to, e.g. `/admin/members`. */
  returnTo: string;
  app: Registration;
}

/** This page, without query or hash: `http://192.168.1.161/app/`. */
export function redirectUri(): string {
  return `${location.origin}${location.pathname}`;
}

function load<T>(storage: Storage, key: string): T | null {
  try {
    const raw = storage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function save(storage: Storage, key: string, value: unknown): void {
  try {
    if (value === null) storage.removeItem(key);
    else storage.setItem(key, JSON.stringify(value));
  } catch {
    // Without storage the flow still works within one page load.
  }
}

@Injectable({ providedIn: 'root' })
export class Auth {
  private readonly api = inject(Api);
  private readonly navigate = inject(OAUTH_NAVIGATE);
  private readonly grant = signal(
    load<{ token: string; scopes: string[] }>(localStorage, GRANT_KEY),
  );

  /** The signed-in member, or null. */
  readonly me = signal<Account | null>(null);
  readonly role = computed<Role | null>(() => {
    const me = this.me();
    return me ? roleOf(me) : null;
  });
  readonly isAdmin = computed(() => this.role() === 'owner' || this.role() === 'admin');
  readonly isOwner = computed(() => this.role() === 'owner');
  readonly canAdminister = computed(() => {
    const grant = this.grant();
    return (
      this.isAdmin() &&
      grant?.token === this.api.token() &&
      grant.scopes.includes('admin:read') &&
      grant.scopes.includes('admin:write')
    );
  });

  constructor() {
    this.api.onUnauthorized = () => this.forget();
  }

  /**
   * At startup: finish a sign-in if the server just redirected back here,
   * then load who is signed in. Returns the route to show, if any.
   */
  async start(): Promise<string | null> {
    const params = new URLSearchParams(location.search);
    let returnTo: string | null = null;
    if (params.has('code') || params.has('error')) {
      // Strip the code from the address bar and history either way.
      history.replaceState(null, '', redirectUri() + location.hash);
      returnTo = await this.finish(params);
    }
    if (this.api.token()) {
      try {
        this.me.set(await this.api.get<Account>('/api/v1/accounts/verify_credentials'));
      } catch (e) {
        if (!(e instanceof ApiError) || e.status === 401) this.forget();
        else throw e;
      }
    }
    return returnTo;
  }

  /** Go to the server's sign-in page. */
  async signIn(returnTo: string, admin = false): Promise<void> {
    const scopes = admin ? ADMIN_SCOPES : SCOPES;
    const app = await this.registration(scopes);
    const pending: Pending = { state: randomToken(), verifier: randomToken(), returnTo, app };
    save(sessionStorage, PENDING_KEY, pending);
    const query = new URLSearchParams({
      response_type: 'code',
      client_id: app.client_id,
      redirect_uri: app.redirect_uri,
      scope: scopes,
      state: pending.state,
      code_challenge: await challengeFor(pending.verifier),
      code_challenge_method: 'S256',
    });
    this.navigate(`/oauth/authorize?${query}`);
  }

  /** Revoke this browser's token and forget it. */
  async signOut(): Promise<void> {
    const token = this.api.token();
    const app = load<Registration>(localStorage, APP_KEY);
    if (token && app) {
      await this.api
        .form('/oauth/revoke', {
          token,
          client_id: app.client_id,
          client_secret: app.client_secret,
        })
        .catch(() => undefined);
    }
    this.forget();
  }

  /** Drop the local session (the server already has, or just did). */
  forget(): void {
    this.api.setToken(null);
    this.grant.set(null);
    save(localStorage, GRANT_KEY, null);
    this.me.set(null);
  }

  private async finish(params: URLSearchParams): Promise<string | null> {
    const pending = load<Pending>(sessionStorage, PENDING_KEY);
    save(sessionStorage, PENDING_KEY, null);
    const app = pending?.app;
    const code = params.get('code');
    if (!pending || !app || params.get('state') !== pending.state) {
      throw new ApiError(
        400,
        'This sign-in expired or belongs to another tab. Please sign in again.',
      );
    }
    if (!code) throw new ApiError(403, 'Authorization was not completed. You can try again.');
    const token = await this.api.form<{ access_token: string; scope: string }>('/oauth/token', {
      grant_type: 'authorization_code',
      code,
      client_id: app.client_id,
      client_secret: app.client_secret,
      redirect_uri: app.redirect_uri,
      code_verifier: pending.verifier,
    });
    const previous = this.api.token();
    this.api.setToken(token.access_token);
    const grant = { token: token.access_token, scopes: (token.scope ?? '').split(' ') };
    this.grant.set(grant);
    save(localStorage, GRANT_KEY, grant);
    if (previous && previous !== token.access_token) {
      await this.api.form('/oauth/revoke', { token: previous }).catch(() => undefined);
    }
    return pending.returnTo;
  }

  /**
   * This browser's app registration, made or remade as needed. The server
   * drops apps without tokens when its 32 slots fill up, so a stored one is
   * checked with a client_credentials grant before use.
   */
  private async registration(scopes: string): Promise<Registration> {
    const stored = load<Registration>(localStorage, APP_KEY);
    if (
      stored &&
      stored.redirect_uri === redirectUri() &&
      scopes.split(' ').every((scope) => stored.scopes?.split(' ').includes(scope))
    ) {
      const ok = await this.api
        .form('/oauth/token', {
          grant_type: 'client_credentials',
          client_id: stored.client_id,
          client_secret: stored.client_secret,
          scope: 'read',
        })
        .then(() => true)
        .catch(() => false);
      if (ok) return stored;
    }
    const app = await this.api.post<{ client_id: string; client_secret: string }>('/api/v1/apps', {
      client_name: 'mastomini household app',
      redirect_uris: redirectUri(),
      scopes,
    });
    const registration = { ...app, redirect_uri: redirectUri(), scopes };
    save(localStorage, APP_KEY, registration);
    return registration;
  }
}
