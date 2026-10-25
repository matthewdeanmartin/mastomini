import { TestBed } from '@angular/core/testing';
import { Api } from '../api/api';
import { SreActions } from './sre-actions';

describe('SRE approvals', () => {
  afterEach(() => TestBed.resetTestingModule());
  it('does no background requests and approves the displayed id and code only on a click', async () => {
    const pending = { id: 'exact-id', code: 'one-use-code', board: 'https://collector.local', action: 'restart-planned', stage: 'pending', not_before_ms: 1800000000000, expires_ms: 1800001800000, summary: 'Dry run passed' };
    const get = vi.fn().mockResolvedValue({pending});
    const post = vi.fn().mockResolvedValue({pending: {...pending, stage: 'approved', code: ''}});
    TestBed.configureTestingModule({providers: [{provide: Api, useValue: {get, post}}]});
    const fixture = TestBed.createComponent(SreActions); fixture.detectChanges();
    expect(get).not.toHaveBeenCalled(); expect(post).not.toHaveBeenCalled();
    fixture.nativeElement.querySelector('button').click();
    await fixture.whenStable(); fixture.detectChanges();
    expect(fixture.nativeElement.textContent).toContain('collector.local');
    expect(fixture.nativeElement.textContent).toContain('UTC');
    const button = Array.from(fixture.nativeElement.querySelectorAll('button')).find(b => (b as HTMLButtonElement).textContent?.includes('Approve this')) as HTMLButtonElement;
    button.click(); await fixture.whenStable(); fixture.detectChanges();
    expect(post).toHaveBeenCalledExactlyOnceWith('/api/v1/bots/sre/actions/approve', {id: 'exact-id', code: 'one-use-code'});
    expect(fixture.nativeElement.textContent).not.toContain('Approve this');
  });
});
