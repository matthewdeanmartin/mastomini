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

describe('API cache policy', () => {
  it('allows only public metadata to use the browser cache', async () => {
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(new Response('{}')));
    vi.stubGlobal('fetch', fetch);
    const api = new Api();
    for (const path of ['/api/v2/instance', '/api/v1/instance/rules', '/api/v1/custom_emojis']) {
      await api.get(path);
      expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'default' }));
    }
    for (const path of [
      '/api/v1/timelines/home',
      '/api/v1/accounts/verify_credentials',
      '/api/mastomini/v1/admin/server',
      '/api/v1/instance/unknown',
    ]) {
      await api.get(path);
      expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'no-store' }));
    }
  });

  it('revalidates after writes and account changes for the full freshness window', async () => {
    let now = 100_000;
    vi.spyOn(Date, 'now').mockImplementation(() => now);
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(new Response('{}')));
    vi.stubGlobal('fetch', fetch);
    const api = new Api();
    const path = '/api/v1/instance/rules';
    await api.put('/api/mastomini/v1/admin/server', { rules: ['New rule'] });
    await api.get(path);
    expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'no-cache' }));
    now += 60_001;
    await api.get(path);
    expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'default' }));
    api.setToken('another-account');
    await api.get(path);
    expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'no-cache' }));
    api.setToken(null);
    await api.get(path);
    expect(fetch).toHaveBeenLastCalledWith(path, expect.objectContaining({ cache: 'no-cache' }));
  });

  it('revalidates while a write is pending and after an uncertain network failure', async () => {
    let fail!: (reason: Error) => void;
    const fetch = vi.fn().mockImplementation((_path, init) =>
      init.method === 'PUT'
        ? new Promise((_resolve, reject) => {
            fail = reject;
          })
        : Promise.resolve(new Response('{}')),
    );
    vi.stubGlobal('fetch', fetch);
    const api = new Api();
    const write = api.put('/api/mastomini/v1/admin/server', {});
    await api.get('/api/v2/instance');
    expect(fetch).toHaveBeenLastCalledWith(
      '/api/v2/instance',
      expect.objectContaining({ cache: 'no-cache' }),
    );
    fail(new Error('connection lost'));
    await expect(write).rejects.toThrow('Could not reach');
    await api.get('/api/v2/instance');
    expect(fetch).toHaveBeenLastCalledWith(
      '/api/v2/instance',
      expect.objectContaining({ cache: 'no-cache' }),
    );
  });
});

it('sends the caller�s idempotency key on scheduled post submissions', async () => {
  const fetch = vi.fn().mockResolvedValue(new Response('{}'));
  vi.stubGlobal('fetch', fetch);
  await new Api().post('/api/v1/statuses', { status: 'later' }, 'stable-submission-key');
  expect(fetch).toHaveBeenCalledWith(
    '/api/v1/statuses',
    expect.objectContaining({
      cache: 'no-store',
      headers: expect.objectContaining({ 'Idempotency-Key': 'stable-submission-key' }),
    }),
  );
});
