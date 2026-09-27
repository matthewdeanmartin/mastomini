import { TestBed } from '@angular/core/testing';
import { afterEach, expect, it, vi } from 'vitest';
import { Api, ApiError } from '../api/api';
import { ScheduledPage } from './scheduled';

afterEach(() => TestBed.resetTestingModule());

it('keeps an uncertain submission retry idempotent and converts local dates to UTC', async () => {
  const api = {
    get: vi.fn().mockResolvedValue([]),
    post: vi.fn().mockRejectedValueOnce(new ApiError(0, 'offline')).mockResolvedValueOnce({ id: '1' }),
  };
  TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: api }] });
  const fixture = TestBed.createComponent(ScheduledPage);
  const page = fixture.componentInstance as unknown as {
    text: string; at: string; schedule(): Promise<void>; error(): string;
  };
  page.text = 'Later';
  page.at = '2030-01-02T10:30';
  await page.schedule();
  expect(page.text).toBe('Later');
  expect(page.error()).toBe('offline');
  await page.schedule();
  expect(api.post.mock.calls[0][2]).toBe(api.post.mock.calls[1][2]);
  expect(api.post.mock.calls[0][1].scheduled_at).toBe(new Date(page.at).toISOString());
  expect(page.text).toBe('');
});

it('changes the submission key when the draft changes after an uncertain failure', async () => {
  const api = { get: vi.fn().mockResolvedValue([]), post: vi.fn().mockRejectedValue(new ApiError(0, 'offline')) };
  TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: api }] });
  const page = TestBed.createComponent(ScheduledPage).componentInstance as unknown as {
    text: string; at: string; schedule(): Promise<void>;
  };
  page.text = 'First'; page.at = '2030-01-02T10:30';
  await page.schedule();
  page.text = 'Changed';
  await page.schedule();
  expect(api.post.mock.calls[0][2]).not.toBe(api.post.mock.calls[1][2]);
});
