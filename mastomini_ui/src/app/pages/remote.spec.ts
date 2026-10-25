import { Type } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { afterEach, expect, it, vi } from 'vitest';
import { Api, ApiError } from '../api/api';
import { Auth } from '../api/auth';
import { feedStatus, RemotePage, remoteOnly, RemoteView } from './remote';
import { RemoteAdminPage, signInMessage } from './remote-admin';

afterEach(() => TestBed.resetTestingModule());

const view = (following: RemoteView['following'] = []): RemoteView => ({
  enabled: true,
  search_server: 'mastodon.nu',
  feeds_max_board: 30,
  feeds_max_account: 10,
  images: false,
  board_count: following.length,
  following,
});

function page<T>(component: Type<T>, api: object): T {
  TestBed.configureTestingModule({
    providers: [
      provideRouter([]),
      { provide: Api, useValue: api },
      { provide: Auth, useValue: { isAdmin: () => false } },
    ],
  });
  return TestBed.createComponent(component).componentInstance;
}

it('searches through the board and keeps only accounts on other servers', async () => {
  const bob = { id: '7', acct: 'bob@example.org', display_name: 'Bob', url: '', avatar: '' };
  const api = {
    get: vi
      .fn()
      .mockResolvedValueOnce(view())
      .mockResolvedValueOnce({ accounts: [{ ...bob, id: '1', acct: 'alice' }, bob] })
      .mockResolvedValueOnce(view([bob])),
    post: vi.fn().mockResolvedValue({ following: true }),
  };
  const p = page(RemotePage, api);
  await p.ngOnInit();
  p.query = ' @bob@example.org ';
  await p.search();
  expect(api.get.mock.calls[1][0]).toBe(
    '/api/v2/search?type=accounts&resolve=true&limit=10&q=%40bob%40example.org',
  );
  expect(p.results().map((a) => a.acct)).toEqual(['bob@example.org']);
  await p.follow(bob);
  expect(api.post).toHaveBeenCalledWith('/api/v1/accounts/7/follow');
  expect(p.following('7')).toBe(true);
});

it('shows why a follow was refused', async () => {
  const api = {
    get: vi.fn().mockResolvedValue(view()),
    post: vi.fn().mockRejectedValue(new ApiError(422, 'You already follow 10 accounts on other servers')),
  };
  const p = page(RemotePage, api);
  await p.ngOnInit();
  await p.follow({ id: '9', acct: 'c@d.org', display_name: '', url: '', avatar: '' });
  expect(p.error()).toContain('already follow 10');
});

it('describes each feed in a line', () => {
  const feed = { url: '', last_ok_at: null, last_try_at: null, failures: 0, error: null, gone: false, posts: 0 };
  expect(feedStatus(feed)).toBe('Posts appear after the next nightly fetch.');
  expect(feedStatus({ ...feed, gone: true })).toContain('gone');
  expect(feedStatus({ ...feed, error: 'timeout' })).toBe('Last try failed: timeout');
  expect(remoteOnly([{ id: '1', acct: 'me', display_name: '', url: '', avatar: '' }])).toEqual([]);
});

it('saves limits as numbers and shows a new device key once', async () => {
  const admin = {
    ...view(),
    feeds_ceiling_board: 64,
    feeds_ceiling_account: 32,
    media_quota_mb: 256,
    media_bytes: 0,
    accounts: [],
    device_keys: [],
    search_account: null,
    search_signed_in: false,
  };
  const api = {
    get: vi.fn().mockResolvedValue(admin),
    put: vi.fn().mockResolvedValue({ ...admin, feeds_max_account: 3 }),
    post: vi.fn().mockResolvedValue({ id: '5', name: 'Bots board', key: 'mmdev_secret' }),
  };
  const p = page(RemoteAdminPage, api);
  await p.ngOnInit();
  p.form.feeds_max_account = 3;
  await p.save();
  expect(api.put.mock.calls[0][1]).toMatchObject({ feeds_max_account: 3, feeds_max_board: 30 });
  expect(p.form.feeds_max_account).toBe(3);
  await p.create();
  expect(p.newKey()).toBe('mmdev_secret');
});

it('sends the admin to the chosen server to sign in, and reports how it went', async () => {
  const admin = {
    ...view(),
    search_server: 'example.social',
    feeds_ceiling_board: 64,
    feeds_ceiling_account: 32,
    media_quota_mb: 256,
    media_bytes: 0,
    accounts: [],
    device_keys: [],
    search_account: null,
    search_signed_in: false,
  };
  const api = {
    get: vi.fn().mockResolvedValue(admin),
    post: vi.fn().mockResolvedValue({ authorize_url: 'https://mastodon.social/oauth/authorize?x' }),
  };
  const p = page(RemoteAdminPage, api);
  await p.ngOnInit();
  expect(p.choice).toBe('other');
  expect(p.chosen()).toBe('example.social');
  p.choice = 'mastodon.social';
  const go = vi.fn();
  p.navigate = go;
  await p.signIn();
  expect(api.post).toHaveBeenCalledWith('/api/mastomini/v1/admin/search_login', { server: 'mastodon.social' });
  expect(go).toHaveBeenCalledWith('https://mastodon.social/oauth/authorize?x');
  expect(signInMessage('connected', null)[0]).toContain('Signed in');
  expect(signInMessage('failed', 'the server said access_denied')[1]).toContain('access_denied');
  expect(signInMessage(null, null)).toEqual(['', '']);
});
