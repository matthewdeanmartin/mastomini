import { TestBed } from '@angular/core/testing';
import { Api } from '../api/api';
import { NanaCoinSetupPage, defaults } from './nanacoin-setup';

describe('NanaCoin setup', () => {
  afterEach(() => { TestBed.resetTestingModule(); vi.useRealTimers(); });
  it('suggests seven separate accounts, six strategies and no implicit grants', () => {
    const bots = defaults();
    expect(bots).toHaveLength(7);
    expect(new Set(bots.map(b => b.username)).size).toBe(7);
    expect(new Set(bots.slice(1).map(b => b.strategy)).size).toBe(6);
    expect(bots.every(b => !b.initial_grant)).toBe(true);
  });
  it('clears passwords immediately and submits only selected bots', async () => {
    let finish!: (value: unknown) => void;
    const post = vi.fn((_path: string, _body: Record<string, unknown>) => new Promise(resolve => finish = resolve));
    TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: { post } }] });
    const page = TestBed.runInInjectionContext(() => new NanaCoinSetupPage());
    page.nanaPassword = 'nana secret'; page.adminPassword = 'admin secret';
    page.selected['trader_1'] = false;
    const task = page.start();
    expect(page.nanaPassword).toBe(''); expect(page.adminPassword).toBe('');
    expect(post.mock.calls[0][1]['bots']).toHaveLength(6);
    expect(post.mock.calls[0][1]['replace_news_read_key']).toBe(false);
    finish({ running: false, rows: [], error: null }); await task;
    expect(page.busy()).toBe(false);
    page.ngOnDestroy();
  });
  it('restores a nonsecret saved plan and does not overlap progress polls', async () => {
    vi.useFakeTimers();
    const get = vi.fn().mockResolvedValue({ running: true, rows: [], error: null, plan: { nanacoin: 'https://nc.test', mastomini: 'https://mm.test', bots: [defaults()[1]] } });
    TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: { get } }] });
    const page = TestBed.runInInjectionContext(() => new NanaCoinSetupPage());
    await page.ngOnInit();
    expect(page.nanacoin).toBe('https://nc.test');
    expect(page.selected['nana_news']).toBe(false);
    let finish!: (value: unknown) => void;
    get.mockImplementationOnce(() => new Promise(resolve => finish = resolve));
    await vi.advanceTimersByTimeAsync(5000);
    expect(get).toHaveBeenCalledTimes(2);
    page.ngOnDestroy(); finish({ running: true, rows: [], error: null }); await Promise.resolve();
    expect(vi.getTimerCount()).toBe(0);
  });
  it('does not restart polling after navigating away during initial load', async () => {
    vi.useFakeTimers();
    let finish!: (value: unknown) => void;
    const get = vi.fn(() => new Promise(resolve => finish = resolve));
    TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: { get } }] });
    const page = TestBed.runInInjectionContext(() => new NanaCoinSetupPage());
    const task = page.ngOnInit(); page.ngOnDestroy();
    finish({ running: true, rows: [], error: null }); await task;
    expect(vi.getTimerCount()).toBe(0);
  });
});
