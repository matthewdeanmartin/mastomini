import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Admin, Announcement } from '../api/admin';
import { ApiError } from '../api/api';
import { Household } from '../api/household';
import { AnnouncementsPage, localDate, utcDate } from './announcements';

const row: Announcement = {
  id: '1',
  text: 'hello',
  content: '<p>hello</p>',
  published: false,
  starts_at: null,
  ends_at: null,
  all_day: false,
  updated_at: '2026-09-26T12:00:00Z',
};
let admin: {
  announcements: ReturnType<typeof vi.fn>;
  saveAnnouncement: ReturnType<typeof vi.fn>;
  publish: ReturnType<typeof vi.fn>;
  deleteAnnouncement: ReturnType<typeof vi.fn>;
};
beforeEach(() => {
  admin = {
    announcements: vi.fn().mockResolvedValue([row]),
    saveAnnouncement: vi.fn().mockResolvedValue(row),
    publish: vi.fn().mockResolvedValue({ ...row, published: true }),
    deleteAnnouncement: vi.fn().mockResolvedValue({}),
  };
  TestBed.configureTestingModule({
    providers: [
      { provide: Admin, useValue: admin },
      { provide: Household, useValue: { status: vi.fn().mockResolvedValue({ clock: 'unset' }) } },
    ],
  });
  vi.spyOn(window, 'confirm').mockReturnValue(true);
});
afterEach(() => {
  TestBed.resetTestingModule();
  vi.restoreAllMocks();
});

describe('announcement administration', () => {
  it('saves an unpublished draft, counts UTF-8 bytes and warns about board time', async () => {
    const fixture = TestBed.createComponent(AnnouncementsPage);
    const page = fixture.componentInstance;
    await page.ngOnInit();
    page.edit(null);
    page.text = '😀';
    expect(page.bytes()).toBe(4);
    await page.save();
    expect(admin.saveAnnouncement).toHaveBeenCalledWith(null, {
      text: '😀',
      published: false,
      all_day: false,
      starts_at: '',
      ends_at: '',
    });
    expect(page.clockWarning()).toContain('unset');
    expect(page.editing()).toBe(false);
  });
  it('rejects oversized drafts and reversed dates without a network mutation', async () => {
    const page = TestBed.createComponent(AnnouncementsPage).componentInstance;
    page.edit(null);
    page.text = '😀'.repeat(513);
    await page.save();
    expect(admin.saveAnnouncement).not.toHaveBeenCalled();
    page.text = 'ok';
    page.start = '2026-10-02T12:00';
    page.end = '2026-10-01T12:00';
    await page.save();
    expect(admin.saveAnnouncement).not.toHaveBeenCalled();
    expect(page.error()).toContain('end');
  });
  it('preserves text and blocks duplicate creation after an ambiguous timeout', async () => {
    admin.saveAnnouncement.mockRejectedValue(new ApiError(0, 'offline'));
    const page = TestBed.createComponent(AnnouncementsPage).componentInstance;
    page.edit(null);
    page.text = 'Keep this draft';
    await page.save();
    await page.save();
    expect(admin.saveAnnouncement).toHaveBeenCalledOnce();
    expect(page.text).toBe('Keep this draft');
    expect(page.uncertain()).toBe(true);
    await page.refresh();
    expect(page.error()).toContain('Inspect');
  });
  it('publishes explicitly and escapes text in the browser preview', async () => {
    const fixture = TestBed.createComponent(AnnouncementsPage);
    fixture.detectChanges();
    await fixture.whenStable();
    const page = fixture.componentInstance;
    await vi.waitFor(() => expect(page.busy()).toBe(false));
    page.edit(null);
    page.text = '<img src=x onerror=alert(1)>';
    fixture.detectChanges();
    expect(fixture.nativeElement.querySelector('.plain-preview img')).toBeNull();
    await page.publish(row);
    expect(admin.publish).toHaveBeenCalledWith('1', true);
  });
  it('round-trips local date inputs and rejects impossible calendar values', () => {
    const local = '2026-09-26T12:30:00';
    expect(localDate(utcDate(local))).toBe(local);
    expect(utcDate('')).toBe('');
    expect(() => utcDate('2026-02-30T12:00')).toThrow('valid');
  });
});
