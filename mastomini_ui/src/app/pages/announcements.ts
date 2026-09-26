import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Admin, Announcement } from '../api/admin';
import { ApiError, describe } from '../api/api';
import { Household } from '../api/household';
import { when } from './format';

export function localDate(iso: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Reject invalid dates and local times skipped by daylight saving transitions. */
export function utcDate(local: string): string {
  if (!local) return '';
  const value = local.length === 16 ? `${local}:00` : local;
  const d = new Date(value);
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/.test(value) ||
    Number.isNaN(d.getTime()) ||
    localDate(d.toISOString()) !== value
  ) {
    throw new ApiError(422, 'Choose a valid local date and time.');
  }
  return d.toISOString();
}

@Component({
  selector: 'app-announcements',
  imports: [FormsModule],
  template: `
    <h1>Announcements</h1>
    <p class="lede">
      Write household notices for connected Mastodon apps. Up to 16 announcements, including drafts.
    </p>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }
    @if (notice()) {
      <p class="ok" role="status">{{ notice() }}</p>
    }
    @if (clockWarning()) {
      <p class="error">{{ clockWarning() }}</p>
    }
    @if (!loaded()) {
      <p role="status">Loading announcements…</p>
    }
    <button class="btn btn--quiet" [disabled]="busy()" (click)="refresh()">Refresh</button>
    <button
      class="btn"
      [disabled]="busy() || !loaded() || rows().length >= 16"
      (click)="edit(null)"
    >
      New draft
    </button>
    @if (loaded()) {
      <p class="muted">{{ rows().length }} / 16 stored</p>
      @if (!rows().length) {
        <p>No announcements yet.</p>
      }
      <ul class="rows">
        @for (row of rows(); track row.id) {
          <li>
            <strong>{{ row.published ? 'Published' : 'Draft' }}</strong>
            <p class="plain-preview">{{ row.text }}</p>
            <p class="small muted">
              Updated {{ when(row.updated_at) }}. Visible from
              {{ row.starts_at ? when(row.starts_at) : 'publication' }} until
              {{ row.ends_at ? when(row.ends_at) : 'unpublished' }}.
              @if (row.all_day) {
                All-day notice.
              }
            </p>
            <button class="btn btn--quiet" [disabled]="busy()" (click)="edit(row)">Edit</button>
            <button class="btn" [disabled]="busy()" (click)="publish(row)">
              {{ row.published ? 'Unpublish' : 'Publish' }}
            </button>
            <button class="btn btn--danger" [disabled]="busy()" (click)="remove(row)">
              Delete
            </button>
          </li>
        }
      </ul>
    }
    @if (editing()) {
      <form class="panel" (ngSubmit)="save()">
        <h2>{{ editingId ? 'Edit announcement' : 'New draft' }}</h2>
        <fieldset [disabled]="busy()">
          <label for="announcement-text">Plain text</label>
          <textarea
            id="announcement-text"
            name="text"
            rows="6"
            required
            [(ngModel)]="text"
          ></textarea>
          <p [class.error]="bytes() > 2048">{{ bytes() }} / 2,048 UTF-8 bytes</p>
          <label for="announcement-start">Visible from ({{ timezone }})</label>
          <input
            id="announcement-start"
            name="start"
            type="datetime-local"
            step="1"
            [(ngModel)]="start"
          />
          <label for="announcement-end">Visible until ({{ timezone }})</label>
          <input
            id="announcement-end"
            name="end"
            type="datetime-local"
            step="1"
            [(ngModel)]="end"
          />
          <p class="small muted">
            Blank dates mean no time limit. Board time controls visibility. For a repeated
            daylight-saving hour, your browser chooses the offset; check the UTC preview below.
          </p>
          <label
            ><input name="allDay" type="checkbox" [(ngModel)]="allDay" /> All-day notice (dates
            still control visibility)</label
          >
          <h2>Text preview</h2>
          <p class="plain-preview">{{ text }}</p>
          <p class="small muted">{{ windowPreview() }}</p>
          <p class="small muted">
            New notices are saved as drafts. Publishing is a separate action. Saving an existing
            published notice updates it immediately.
          </p>
          <button
            class="btn"
            type="submit"
            [disabled]="!text.trim() || bytes() > 2048 || uncertain()"
          >
            {{ editingId ? 'Save changes' : 'Save draft' }}
          </button>
          <button class="btn btn--quiet" type="button" (click)="cancel()">Cancel</button>
        </fieldset>
      </form>
    }
  `,
})
export class AnnouncementsPage implements OnInit {
  private readonly admin = inject(Admin);
  private readonly household = inject(Household);
  readonly rows = signal<Announcement[]>([]);
  readonly busy = signal(false);
  readonly loaded = signal(false);
  readonly editing = signal(false);
  readonly uncertain = signal(false);
  readonly error = signal('');
  readonly notice = signal('');
  readonly clockWarning = signal('');
  readonly timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  readonly when = when;
  editingId: string | null = null;
  text = '';
  start = '';
  end = '';
  allDay = false;
  private original = '';

  async ngOnInit(): Promise<void> {
    await this.refresh();
  }
  bytes(): number {
    return new TextEncoder().encode(this.text.trim()).length;
  }
  windowPreview(): string {
    try {
      return `UTC: ${utcDate(this.start) || 'no start limit'} — ${utcDate(this.end) || 'no end limit'}`;
    } catch {
      return 'Choose valid dates to see the UTC window.';
    }
  }
  private snapshot(): string {
    return JSON.stringify([this.text, this.start, this.end, this.allDay]);
  }
  private discard(): boolean {
    return (
      !this.editing() ||
      this.original === this.snapshot() ||
      confirm('Discard unsaved announcement changes?')
    );
  }
  edit(row: Announcement | null): void {
    if (this.busy() || !this.discard()) return;
    this.editingId = row?.id ?? null;
    this.text = row?.text ?? '';
    this.start = localDate(row?.starts_at ?? null);
    this.end = localDate(row?.ends_at ?? null);
    this.allDay = row?.all_day ?? false;
    this.original = this.snapshot();
    this.editing.set(true);
    this.uncertain.set(false);
    this.error.set('');
  }
  cancel(): void {
    if (this.discard()) this.editing.set(false);
  }
  canLeave(): boolean {
    return !this.busy() && this.discard();
  }
  async refresh(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    try {
      this.rows.set(await this.admin.announcements());
      this.loaded.set(true);
      // An ambiguous create is not retried automatically. Inspect the list first.
      if (this.uncertain())
        this.error.set(
          'Inspect the refreshed list before starting another draft. Your unsaved text is still below.',
        );
      try {
        const status = await this.household.status();
        this.clockWarning.set(
          status.clock === 'unset'
            ? 'The board clock is unset. Set it on the Health page before saving or scheduling notices.'
            : '',
        );
      } catch {
        this.clockWarning.set(
          'Could not verify board time. Check the Health page before scheduling notices.',
        );
      }
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
  async save(): Promise<void> {
    if (this.busy() || this.uncertain()) return;
    this.error.set('');
    this.notice.set('');
    let starts_at: string, ends_at: string;
    try {
      starts_at = utcDate(this.start);
      ends_at = utcDate(this.end);
      if (!this.text.trim() || this.bytes() > 2048)
        throw new ApiError(422, 'Enter 1–2,048 UTF-8 bytes of text.');
      if (starts_at && ends_at && starts_at > ends_at)
        throw new ApiError(422, 'The end must not precede the start.');
    } catch (e) {
      this.error.set(describe(e));
      return;
    }
    this.busy.set(true);
    try {
      const row = await this.admin.saveAnnouncement(this.editingId, {
        text: this.text.trim(),
        ...(this.editingId ? {} : { published: false }),
        all_day: this.allDay,
        starts_at,
        ends_at,
      });
      this.rows.update((rows) => [row, ...rows.filter((r) => r.id !== row.id)]);
      this.editing.set(false);
      this.notice.set(row.published ? 'Published announcement updated.' : 'Draft saved.');
    } catch (e) {
      if (!(e instanceof ApiError) || e.status === 0 || e.status >= 500) {
        this.uncertain.set(true);
        this.error.set(
          'The result is uncertain. Refresh and inspect the list before retrying. Your text has been kept.',
        );
      } else this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
  async publish(row: Announcement): Promise<void> {
    if (this.busy() || !confirm(`${row.published ? 'Unpublish' : 'Publish'} this announcement?`))
      return;
    if (this.editing() && this.editingId === row.id && !this.discard()) return;
    await this.mutate(async () => {
      const updated = await this.admin.publish(row.id, !row.published);
      this.rows.update((rows) => rows.map((r) => (r.id === row.id ? updated : r)));
      if (this.editingId === row.id) this.editing.set(false);
    });
  }
  async remove(row: Announcement): Promise<void> {
    if (this.busy() || !confirm('Permanently delete this announcement and its reactions?')) return;
    if (this.editing() && this.editingId === row.id && !this.discard()) return;
    await this.mutate(async () => {
      await this.admin.deleteAnnouncement(row.id);
      this.rows.update((rows) => rows.filter((r) => r.id !== row.id));
      if (this.editingId === row.id) this.editing.set(false);
    });
  }
  private async mutate(action: () => Promise<void>): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    this.notice.set('');
    try {
      await action();
      this.notice.set('Announcement updated.');
    } catch (e) {
      this.error.set(`${describe(e)} Refresh to check the current state before retrying.`);
    } finally {
      this.busy.set(false);
    }
  }
}
