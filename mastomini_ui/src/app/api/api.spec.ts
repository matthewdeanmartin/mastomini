import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Api, pageLinks } from './api';

beforeEach(() => localStorage.clear());
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('API pagination', () => {
  it('reads Link relations without losing filters or large string IDs', () => {
    const path = '/api/v1/admin/reports?resolved=true&limit=10';
    const next = '/api/v1/admin/reports?resolved=true&max_id=999999999999999999&limit=10';
    expect(
      pageLinks(`<${location.origin}${next}>; rel="next", <?min_id=1>; rel="prev"`, path),
    ).toEqual({ next, prev: '/api/v1/admin/reports?min_id=1' });
  });
  it('ignores cross-origin, credential-bearing and unrelated-resource links', () => {
    const path = '/api/v1/admin/reports';
    for (const link of [
      'https://evil.invalid/api/v1/admin/reports',
      '//evil.invalid/api/v1/admin/reports',
      '/api/v1/admin/accounts',
      `${location.origin.replace('://', '://user:pw@')}${path}`,
      'http://[bad',
    ]) {
      expect(pageLinks(`<${link}>; rel="next"`, path).next).toBeNull();
    }
    expect(pageLinks(null, path)).toEqual({ next: null, prev: null });
  });
  it('exposes pages and honors authentication and cancellation', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response('[{"id":"7"}]', {
        headers: { Link: '</api/v1/admin/reports?max_id=7>; rel="next"' },
      }),
    );
    vi.stubGlobal('fetch', fetch);
    const api = new Api();
    api.setToken('secret');
    const signal = new AbortController().signal;
    expect(await api.page('/api/v1/admin/reports', signal)).toEqual({
      items: [{ id: '7' }],
      next: '/api/v1/admin/reports?max_id=7',
      prev: null,
    });
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/admin/reports',
      expect.objectContaining({
        signal,
        redirect: 'error',
        headers: { Accept: 'application/json', Authorization: 'Bearer secret' },
      }),
    );
    await expect(api.page('https://evil.invalid/api/v1/admin/reports')).rejects.toThrow('outside');
    expect(fetch).toHaveBeenCalledTimes(1);
  });
  it('clears unauthorized sessions and surfaces server errors', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response('{"error":"expired"}', { status: 401 })),
    );
    const api = new Api();
    api.setToken('old');
    api.onUnauthorized = vi.fn();
    await expect(api.page('/api/v1/admin/reports')).rejects.toThrow('expired');
    expect(api.onUnauthorized).toHaveBeenCalledOnce();
  });
});
