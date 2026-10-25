import { TestBed } from '@angular/core/testing';
import { Api } from '../api/api';
import { SreHistory } from './sre-history';

describe('SRE history', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('loads only on demand and renders unknown observations separately from OK', async () => {
    const get = vi.fn().mockResolvedValue({
      hosts: ['a.local', 'b.local'],
      days: [{ day: 1, at_ms: 86400000, statuses: 'o-' }],
    });
    TestBed.configureTestingModule({ providers: [{ provide: Api, useValue: { get } }] });
    const fixture = TestBed.createComponent(SreHistory);
    fixture.detectChanges();
    expect(get).not.toHaveBeenCalled();
    fixture.nativeElement.querySelector('button').click();
    await fixture.whenStable();
    fixture.detectChanges();
    expect(get).toHaveBeenCalledWith('/api/v1/bots/sre/history');
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('a.local');
    expect(text).toContain('OK');
    expect(text).toContain('Not observed');
    expect(text).toContain('UTC');
  });
});
