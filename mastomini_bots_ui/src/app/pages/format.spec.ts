import { describe, expect, it } from 'vitest';

import { bytes, duration, relative, when } from './format';

const NOW = Date.parse('2026-09-26T11:00:00Z');

describe('format', () => {
  it('says how far away a time is', () => {
    expect(relative('2026-09-26T11:30:00Z', NOW)).toBe('in 30 min');
    expect(relative('2026-09-26T14:00:00Z', NOW)).toBe('in 3 h');
    expect(relative('2026-09-26T10:59:50Z', NOW)).toBe('just now');
    expect(relative('2026-09-24T11:00:00Z', NOW)).toBe('2 days ago');
  });

  it('shows nothing for no time', () => {
    expect(when(null, NOW)).toBe('—');
    expect(when('2026-09-26T11:30:00Z', NOW)).toContain('(in 30 min)');
  });

  it('sizes and durations', () => {
    expect(bytes(2048)).toBe('2 KiB');
    expect(bytes(3 * 1024 * 1024)).toBe('3.0 MiB');
    expect(bytes(null)).toBe('—');
    expect(duration(90 * 60_000)).toBe('1 h 30 min');
    expect(duration(3 * 24 * 3_600_000)).toBe('3 days 0 h');
  });
});
