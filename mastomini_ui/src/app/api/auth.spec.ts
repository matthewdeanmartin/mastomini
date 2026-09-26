import { signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Api } from './api';
import { Auth, OAUTH_NAVIGATE } from './auth';

const owner = {
  id: '1',
  username: 'alice',
  display_name: 'Alice',
  role: { id: '0', name: 'Owner' },
};
const credentials = { client_id: 'new-app', client_secret: 'client-secret' };
let api: {
  token: ReturnType<typeof signal<string | null>>;
  setToken: ReturnType<typeof vi.fn>;
  post: ReturnType<typeof vi.fn>;
  form: ReturnType<typeof vi.fn>;
  get: ReturnType<typeof vi.fn>;
};
let navigate: ReturnType<typeof vi.fn>;
beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  history.replaceState(null, '', '/app/');
  const token = signal<string | null>('old-token');
  api = {
    token,
    setToken: vi.fn((v: string | null) => token.set(v)),
    post: vi.fn().mockResolvedValue(credentials),
    form: vi.fn().mockResolvedValue({}),
    get: vi.fn().mockResolvedValue(owner),
  };
  navigate = vi.fn();
  TestBed.configureTestingModule({
    providers: [
      { provide: Api, useValue: api },
      { provide: OAUTH_NAVIGATE, useValue: navigate },
    ],
  });
});
afterEach(() => {
  TestBed.resetTestingModule();
  vi.restoreAllMocks();
});

describe('admin OAuth authorization', () => {
  it('replaces a legacy registration before requesting admin scopes, then keeps granted scopes with the token', async () => {
    localStorage.setItem(
      'mastomini.app',
      JSON.stringify({
        client_id: 'old',
        client_secret: 'old',
        redirect_uri: `${location.origin}/app/`,
      }),
    );
    const auth = TestBed.inject(Auth);
    await auth.start();
    expect(auth.canAdminister()).toBe(false);
    await auth.signIn('/admin/moderation', true);
    expect(api.post).toHaveBeenCalledWith(
      '/api/v1/apps',
      expect.objectContaining({ scopes: 'read write admin:read admin:write' }),
    );
    const url = new URL(navigate.mock.calls[0][0], location.origin);
    expect(url.searchParams.get('scope')).toBe('read write admin:read admin:write');
    expect(url.searchParams.get('code_challenge_method')).toBe('S256');
    api.form.mockResolvedValueOnce({
      access_token: 'admin-token',
      scope: 'read write admin:read admin:write',
    });
    history.replaceState(null, '', `/app/?code=code&state=${url.searchParams.get('state')}`);
    expect(await auth.start()).toBe('/admin/moderation');
    expect(auth.canAdminister()).toBe(true);
    expect(api.form).toHaveBeenCalledWith('/oauth/revoke', { token: 'old-token' });
    expect(location.search).toBe('');
    auth.forget();
    expect(auth.canAdminister()).toBe(false);
    expect(localStorage.getItem('mastomini.grant')).toBeNull();
  });
  it('reuses a compatible registration while ordinary sign-in requests no admin permissions', async () => {
    localStorage.setItem(
      'mastomini.app',
      JSON.stringify({
        ...credentials,
        redirect_uri: `${location.origin}/app/`,
        scopes: 'read write admin:read admin:write',
      }),
    );
    const auth = TestBed.inject(Auth);
    await auth.signIn('/me');
    expect(api.post).not.toHaveBeenCalled();
    expect(new URL(navigate.mock.calls[0][0], location.origin).searchParams.get('scope')).toBe(
      'read write',
    );
  });
  it('does not infer token permissions from app registration or accept a mismatched callback', async () => {
    const auth = TestBed.inject(Auth);
    await auth.start();
    await auth.signIn('/admin/announcements', true);
    history.replaceState(null, '', '/app/?code=code&state=wrong');
    await expect(auth.start()).rejects.toThrow('another tab');
    expect(api.setToken).not.toHaveBeenCalled();
    expect(auth.canAdminister()).toBe(false);
  });
  it('requires both granted scopes and the current admin role', async () => {
    localStorage.setItem(
      'mastomini.grant',
      JSON.stringify({ token: 'old-token', scopes: ['admin:read', 'admin:write'] }),
    );
    api.get.mockResolvedValue({ ...owner, role: { id: '1', name: 'Member' } });
    const auth = TestBed.inject(Auth);
    await auth.start();
    expect(auth.canAdminister()).toBe(false);
  });
});
