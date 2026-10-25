import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { Api } from '../api/api';
import { Session } from '../api/session';
import { Overview, SrePage, overrides } from './sre';

const overview: Overview = {
  boards: [
    { host: 'meritledger.local', url: 'https://meritledger.local', status: 'ok', boot_ms: 86_400_000,
      resets: [{ at_ms: 86_400_000, kind: 'crash' }, { at_ms: 1, kind: 'power on (unplugged)' }],
      heap_kib: 12, heap_usual_kib: 12, largest_kib: 6, largest_usual_kib: null, rssi: -77, rssi_usual: -76, readings: 2 },
  ],
  findings: [{ host: 'meritledger.local', rule: 'heap.low', since_ms: 0, action: 'notify', todo: 'Watch for a leak.' }],
  observations: [],
  rules: [
    { id: 'heap.low', default: 'notify', action: 'digest', todo: '' },
    { id: 'wifi.weak', default: 'digest', action: 'digest', todo: '' },
  ],
  notify: { channels: ['ntfy'], log: ['2026-10-24T07:30:00Z ntfy ok'] },
  record: { bytes: 3600, limit: 4000 },
};

describe('SRE page', () => {
  afterEach(() => TestBed.resetTestingModule());

  async function render() {
    const get = vi.fn().mockResolvedValue(overview);
    const update = vi.fn().mockResolvedValue({});
    const bots = vi.fn().mockResolvedValue([{ id: 'sre', settings: [], ready: true }]);
    TestBed.configureTestingModule({
      providers: [provideRouter([]), { provide: Api, useValue: { get } }, { provide: Session, useValue: { bots, update } }],
    });
    const fixture = TestBed.createComponent(SrePage);
    fixture.detectChanges();
    await fixture.whenStable();
    await new Promise((r) => setTimeout(r));
    fixture.detectChanges();
    return { fixture, get, update };
  }

  it('shows each board against its own usual and warns near the record limit', async () => {
    const { fixture, get } = await render();
    expect(get).toHaveBeenCalledWith('/api/v1/bots/sre/overview');
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('12 KiB, usually 12 KiB');
    expect(text).toContain('6 KiB (learning, 2 of 3)');
    expect(text).toContain('crash');
    expect(text).toContain('(2 this week)');
    expect(text).toContain('3600 of 4000 bytes');
  });

  it('saves only the rules that differ from their defaults', async () => {
    expect(overrides(overview.rules, {})).toBe('heap.low = digest');
    expect(overrides(overview.rules, { 'heap.low': 'notify', 'wifi.weak': 'ignore' })).toBe('wifi.weak = ignore');
    const { fixture, update } = await render();
    const tabs: HTMLButtonElement[] = Array.from(fixture.nativeElement.querySelectorAll('nav.tabs button'));
    tabs.find((b) => b.textContent?.includes('Runbooks'))!.click();
    fixture.detectChanges();
    const reset: HTMLButtonElement = Array.from<HTMLButtonElement>(fixture.nativeElement.querySelectorAll('button'))
      .find((b) => b.textContent?.includes('Reset all'))!;
    reset.click();
    await fixture.whenStable();
    expect(update).toHaveBeenCalledWith('sre', { settings: { runbook_rules: '' } });
  });
});
