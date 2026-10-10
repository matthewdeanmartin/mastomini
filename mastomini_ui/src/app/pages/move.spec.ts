import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api/api';
import { Household } from '../api/household';
import { MovePage } from './move';

describe('MovePage', () => {
  let exportHousehold: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    exportHousehold = vi.fn();
    TestBed.configureTestingModule({
      providers: [{ provide: Household, useValue: { exportHousehold } }],
    });
  });

  afterEach(() => {
    TestBed.resetTestingModule();
    vi.restoreAllMocks();
  });

  function page(): any {
    const fixture = TestBed.createComponent(MovePage);
    fixture.detectChanges();
    return fixture;
  }

  it('checks the passphrase before asking the board', async () => {
    const fixture = page();
    const p = fixture.componentInstance;
    p.password = 'pw';
    p.passphrase = 'short';
    p.again = 'short';
    await p.exportHousehold();
    expect(p.error()).toContain('at least 12');
    p.passphrase = 'correct horse battery';
    p.again = 'correct horse batterz';
    await p.exportHousehold();
    expect(p.error()).toContain('different');
    expect(exportHousehold).not.toHaveBeenCalled();
  });

  it('downloads the export and explains the same-name and new-name moves', async () => {
    exportHousehold.mockResolvedValue({ blob: new Blob(['x']), filename: 'mastomini.local-2026-10-09.mmexport' });
    const create = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:export');
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    const fixture = page();
    const p = fixture.componentInstance;
    p.password = 'alicepw';
    p.passphrase = 'correct horse battery';
    p.again = 'correct horse battery';
    await p.exportHousehold();
    fixture.detectChanges();

    expect(exportHousehold).toHaveBeenCalledWith('alicepw', 'correct horse battery');
    expect(create).toHaveBeenCalled();
    expect(click).toHaveBeenCalledOnce();
    const text = fixture.nativeElement.textContent;
    expect(text).toContain('Saved mastomini.local-2026-10-09.mmexport');
    expect(text).toContain('Same name as this board');
    expect(text).toContain('signs in again');
    expect(p.password).toBe('');
    expect(p.passphrase).toBe('');
  });

  it('shows what the board said when it refuses', async () => {
    exportHousehold.mockRejectedValue(new ApiError(403, 'Wrong password'));
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    const p = page().componentInstance;
    p.password = 'nope';
    p.passphrase = 'correct horse battery';
    p.again = 'correct horse battery';
    await p.exportHousehold();
    expect(p.error()).toBe('Wrong password');
    expect(click).not.toHaveBeenCalled();
    expect(p.busy()).toBe(false);
  });
});
