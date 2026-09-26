// The one place requests are made. Same origin always: the board serves this
// app, and `npm start` proxies /api and /oauth to a server (proxy.conf.mjs).
//
// Plain fetch, no HttpClient: a handful of JSON calls do not need interceptors,
// and every kilobyte of the bundle lives in the board's flash.

import { Injectable, signal } from '@angular/core';

const TOKEN_KEY = 'mastomini.token';

/** The server's `{"error": "..."}`, with the HTTP status. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Private windows may refuse storage; the session then lasts one page.
  }
}

export type Body = Record<string, unknown>;

export interface Page<T> {
  items: T[];
  next: string | null;
  prev: string | null;
}

/** Only follow links to the same API resource, never forward a bearer elsewhere. */
export function pageLinks(header: string | null, path: string): Pick<Page<never>, 'next' | 'prev'> {
  const result = { next: null as string | null, prev: null as string | null };
  const base = new URL(path, location.origin);
  for (const match of (header ?? '').matchAll(/<([^>]+)>\s*((?:;\s*[^,]*)*)/g)) {
    const rel = /;\s*rel\s*=\s*(?:"([^"]+)"|([^;\s,]+))/i.exec(match[2]);
    if (!rel) continue;
    try {
      const url = new URL(match[1], base);
      if (
        url.origin !== location.origin ||
        url.pathname !== base.pathname ||
        url.username ||
        url.password ||
        url.hash
      )
        continue;
      for (const name of (rel[1] ?? rel[2]).split(/\s+/)) {
        if (name === 'next' || name === 'prev') result[name] = url.pathname + url.search;
      }
    } catch {
      /* Ignore malformed links. */
    }
  }
  return result;
}

@Injectable({ providedIn: 'root' })
export class Api {
  /** The bearer token, or null when signed out. */
  readonly token = signal<string | null>(read(TOKEN_KEY));

  /** Fired when the server stops accepting the token (401). */
  onUnauthorized: () => void = () => {};

  setToken(token: string | null): void {
    write(TOKEN_KEY, token);
    this.token.set(token);
  }

  get<T>(path: string): Promise<T> {
    return this.request<T>('GET', path);
  }

  async page<T>(path: string, signal?: AbortSignal): Promise<Page<T>> {
    const authed = this.token() !== null;
    const response = await this.send('GET', path, undefined, signal);
    const items = await this.parse<T[]>(response, authed);
    return { items, ...pageLinks(response.headers.get('Link'), path) };
  }

  post<T>(path: string, body: Body = {}): Promise<T> {
    return this.request<T>('POST', path, body);
  }

  put<T>(path: string, body: Body): Promise<T> {
    return this.request<T>('PUT', path, body);
  }

  delete<T>(path: string, body?: Body): Promise<T> {
    return this.request<T>('DELETE', path, body);
  }

  /** `application/x-www-form-urlencoded`, as OAuth endpoints expect. */
  async form<T>(path: string, fields: Record<string, string>): Promise<T> {
    const response = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams(fields).toString(),
    });
    return this.parse<T>(response, false);
  }

  private async request<T>(method: string, path: string, body?: Body): Promise<T> {
    const authed = this.token() !== null;
    return this.parse<T>(await this.send(method, path, body), authed);
  }

  private async send(
    method: string,
    path: string,
    body?: Body,
    signal?: AbortSignal,
  ): Promise<Response> {
    const url = new URL(path, location.origin);
    if (
      url.origin !== location.origin ||
      !url.pathname.startsWith('/api/') ||
      url.username ||
      url.password
    ) {
      throw new ApiError(0, 'Refusing a request outside this server’s API.');
    }
    const headers: Record<string, string> = { Accept: 'application/json' };
    const token = this.token();
    if (token) headers['Authorization'] = `Bearer ${token}`;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    let response: Response;
    try {
      response = await fetch(url.pathname + url.search, {
        method,
        headers,
        signal,
        redirect: 'error',
        cache: 'no-store',
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      if (signal?.aborted) throw error;
      throw new ApiError(
        0,
        'Could not reach the server. Is the board on, and are you on the home Wi-Fi?',
      );
    }
    return response;
  }

  private async parse<T>(response: Response, authed: boolean): Promise<T> {
    const text = await response.text();
    let json: unknown = null;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      json = null;
    }
    if (response.ok) return json as T;
    const message =
      (json as { error?: string; error_description?: string } | null)?.error_description ??
      (json as { error?: string } | null)?.error ??
      `The server answered ${response.status}.`;
    if (response.status === 401 && authed) this.onUnauthorized();
    throw new ApiError(response.status, message);
  }
}

/** A message fit to show a person. */
export function describe(error: unknown): string {
  if (error instanceof ApiError) return error.message.replace(/^Validation failed: /, '');
  return 'Something went wrong. Try again.';
}
