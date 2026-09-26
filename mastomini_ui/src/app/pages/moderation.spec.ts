import { signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Admin, Report } from '../api/admin';
import { ApiError } from '../api/api';
import { Auth } from '../api/auth';
import { Household } from '../api/household';
import { ModerationPage } from './moderation';

const report: Report = {
  id: '10',
  created_at: '2026-09-26T12:00:00Z',
  updated_at: '',
  action_taken: false,
  action_taken_at: null,
  category: 'other',
  comment: '<script>bad</script>',
  account: null,
  assigned_account: null,
  action_taken_by_account: null,
  rules: [],
  target_account: {
    id: '2',
    username: 'bob',
    account: { id: '2', username: 'bob', display_name: '' },
    disabled: false,
    silenced: false,
    suspended: false,
    sensitized: false,
  },
  statuses: [
    {
      id: '3',
      content: '<img src=x onerror="alert(1)"><script>bad</script>',
      spoiler_text: '',
      account: { id: '2', username: 'bob', display_name: '' },
    },
  ],
};
let admin: Record<string, ReturnType<typeof vi.fn>>;
beforeEach(() => {
  admin = {
    reports: vi.fn().mockResolvedValue({
      items: [report],
      next: '/api/v1/admin/reports?max_id=10&limit=10&resolved=false',
      prev: null,
    }),
    report: vi.fn().mockResolvedValue(report),
    rules: vi.fn().mockResolvedValue([]),
    reportAction: vi.fn().mockResolvedValue(report),
    accountAction: vi.fn().mockResolvedValue({}),
    deleteStatus: vi.fn().mockResolvedValue({}),
    classify: vi.fn().mockResolvedValue(report),
  };
  TestBed.configureTestingModule({
    providers: [
      provideRouter([]),
      { provide: Admin, useValue: admin },
      { provide: Auth, useValue: { me: signal({ id: '1' }) } },
      { provide: Household, useValue: { members: vi.fn().mockResolvedValue([]) } },
    ],
  });
  vi.spyOn(window, 'confirm').mockReturnValue(true);
});
afterEach(() => {
  TestBed.resetTestingModule();
  vi.restoreAllMocks();
});

describe('moderation workflows', () => {
  it('uses server pagination and full-queue filters', async () => {
    const page = TestBed.createComponent(ModerationPage).componentInstance;
    await page.ngOnInit();
    const next = page.next()!;
    await page.load(next);
    expect(admin['reports']).toHaveBeenLastCalledWith(next, expect.any(AbortSignal));
    page.resolved = 'true';
    page.target = '2';
    await page.filter();
    expect(admin['reports']).toHaveBeenLastCalledWith(
      '/api/v1/admin/reports?limit=10&resolved=true&target_account_id=2',
      expect.any(AbortSignal),
    );
  });
  it('does not auto-resolve a case when acting on its member', async () => {
    const page = TestBed.createComponent(ModerationPage).componentInstance;
    await page.open('10');
    await page.accountAction('suspend');
    expect(admin['accountAction']).toHaveBeenCalledWith('2', 'suspend');
    expect(admin['reportAction']).not.toHaveBeenCalled();
    expect(page.notice()).toContain('before resolving');
  });
  it('distinguishes committed actions from failed refresh and prevents stale follow-up actions', async () => {
    const page = TestBed.createComponent(ModerationPage).componentInstance;
    await page.open('10');
    admin['report'].mockRejectedValue(new ApiError(0, 'offline'));
    await page.reportAction('resolve');
    expect(page.error()).toContain('Report updated. Refresh failed');
    expect(page.selected()).toBeNull();
    await page.reportAction('resolve');
    expect(admin['reportAction']).toHaveBeenCalledOnce();
  });
  it('requires confirmation for deletion and sanitizes reported HTML', async () => {
    const fixture = TestBed.createComponent(ModerationPage);
    fixture.detectChanges();
    await fixture.whenStable();
    const page = fixture.componentInstance;
    await page.open('10');
    fixture.detectChanges();
    expect(fixture.nativeElement.querySelector('script')).toBeNull();
    expect(fixture.nativeElement.querySelector('[onerror]')).toBeNull();
    vi.mocked(window.confirm).mockReturnValue(false);
    await page.deletePost('3');
    expect(admin['deleteStatus']).not.toHaveBeenCalled();
  });
});
