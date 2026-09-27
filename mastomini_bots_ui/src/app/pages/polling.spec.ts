import { TestBed } from '@angular/core/testing';
import { Session } from '../api/session';
import { BotsPage } from './bots';
import { ActivityPage } from './activity';

for (const Page of [BotsPage, ActivityPage]) describe(`${Page.name} polling`, () => {
  afterEach(() => { TestBed.resetTestingModule(); vi.useRealTimers(); vi.restoreAllMocks(); });
  it('does not start a timer after leaving during the first request', async () => {
    vi.useFakeTimers();
    let finish!: (value: never[]) => void;
    const pending = new Promise<never[]>(resolve => finish = resolve);
    const request = vi.fn(() => pending);
    TestBed.configureTestingModule({providers:[{provide:Session,useValue:{bots:request,activity:request,openRouter:async()=>({})}}]});
    const page = TestBed.runInInjectionContext(() => new Page());
    const start = page.ngOnInit();
    page.ngOnDestroy(); finish([]); await start;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(request).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });
  it('skips hidden tabs and never overlaps slow polls', async () => {
    vi.useFakeTimers();
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    const request = vi.fn<() => Promise<never[]>>().mockResolvedValue([]);
    TestBed.configureTestingModule({providers:[{provide:Session,useValue:{bots:request,activity:request,openRouter:async()=>({})}}]});
    const page = TestBed.runInInjectionContext(() => new Page());
    await page.ngOnInit();
    hidden.mockReturnValue(true); await vi.advanceTimersByTimeAsync(30_000);
    expect(request).toHaveBeenCalledTimes(1);
    hidden.mockReturnValue(false);
    let finish!: (value: never[]) => void;
    request.mockImplementationOnce(() => new Promise(resolve => finish = resolve));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(request).toHaveBeenCalledTimes(2);
    page.ngOnDestroy(); finish([]); await Promise.resolve();
    expect(vi.getTimerCount()).toBe(0);
  });
});
