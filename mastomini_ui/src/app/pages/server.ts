import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { Api, describe } from '../api/api';
import { Household } from '../api/household';

/** Name, description, rules and terms (`/admin/server`). */
@Component({
  selector: 'app-server',
  imports: [FormsModule],
  template: `
    <h1>Server</h1>
    <p class="lede">
      Shown on the about page and in Mastodon apps. The privacy policy is generated from what the
      server stores and can't be edited.
    </p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (saved()) {
      <p class="ok">Saved.</p>
    }
    @if (loaded()) {
      <form class="panel" (ngSubmit)="save()">
        <label for="title">Household name</label>
        <input id="title" name="title" type="text" maxlength="40" [(ngModel)]="title" required />
        <label for="description">Description</label>
        <textarea id="description" name="description" rows="3" [(ngModel)]="description"></textarea>
        <label for="rules">Rules, one per line (up to 8, each up to 140 bytes)</label>
        <textarea id="rules" name="rules" rows="6" [(ngModel)]="rules"></textarea>
        <label for="terms">Terms of service (plain text, up to 3,000 bytes)</label>
        <textarea id="terms" name="terms" rows="10" [(ngModel)]="terms"></textarea>
        <p class="muted small">
          @if (customized()) {
            Custom terms, in effect since {{ effective() }}. Empty the box to go back to the
            generated terms.
          } @else {
            These are the generated terms, built from the name and rules. Editing them makes them
            custom; saving changed terms resets their effective date.
          }
        </p>
        <button class="btn" type="submit" [disabled]="busy()">Save</button>
      </form>
    }
    <h2>Scheduled posts</h2>
    <p>
      The bots board handles publication times. Use the same mastomini API key already saved on a
      bot there; the scheduler registers automatically when it receives its first post.
    </p>
    <form class="panel" (ngSubmit)="saveScheduler()">
      <label for="bots-url">Bots board address</label>
      <input
        id="bots-url"
        name="botsUrl"
        type="url"
        [(ngModel)]="botsUrl"
        placeholder="https://mastomini-bots.local"
        required
      />
      <label for="scheduler-key">Existing mastomini API key</label>
      <input
        id="scheduler-key"
        name="schedulerKey"
        type="password"
        autocomplete="off"
        [(ngModel)]="schedulerKey"
        required
      />
      <p class="muted">
        Saving authorizes this trusted bots board to publish members� queued posts when due. The key
        must allow posting. Overdue posts publish at the first opportunity.
      </p>
      @if (scheduler(); as state) {
        <p>
          {{ state.key_set ? 'An API key is configured.' : 'Not configured yet.' }} Waiting to hand
          off: {{ state.pending_handoff }}.
        </p>
        @if (state.last_error) {
          <p class="error">{{ state.last_error }}</p>
        }
      }
      <button class="btn" type="submit" [disabled]="busy()">Save scheduler</button>
      <button class="btn" type="button" (click)="refreshScheduler()" [disabled]="busy()">
        Refresh status
      </button>
    </form>
  `,
})
export class ServerPage implements OnInit {
  private readonly household = inject(Household);
  private readonly api = inject(Api);
  protected readonly scheduler = signal<{
    bots_url: string;
    key_set: boolean;
    pending_handoff: number;
    last_error: string | null;
  } | null>(null);
  protected botsUrl = 'https://mastomini-bots.local';
  protected schedulerKey = '';

  protected async refreshScheduler(): Promise<void> {
    try {
      const state = await this.api.get<NonNullable<ReturnType<typeof this.scheduler>>>(
        '/api/mastomini/v1/admin/scheduler',
      );
      this.scheduler.set(state);
      if (state.bots_url) this.botsUrl = state.bots_url;
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async saveScheduler(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    this.saved.set(false);
    try {
      await this.api.put('/api/mastomini/v1/admin/scheduler', {
        bots_url: this.botsUrl,
        api_key: this.schedulerKey,
      });
      this.schedulerKey = '';
      await this.refreshScheduler();
      this.saved.set(true);
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
  protected readonly loaded = signal(false);
  protected readonly busy = signal(false);
  protected readonly saved = signal(false);
  protected readonly error = signal('');
  protected readonly customized = signal(false);
  protected readonly effective = signal('');
  protected title = '';
  protected description = '';
  protected rules = '';
  protected terms = '';
  private originalTerms = '';

  async ngOnInit(): Promise<void> {
    try {
      this.apply(await this.household.server());
      this.loaded.set(true);
      await this.refreshScheduler();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  private apply(s: Awaited<ReturnType<Household['server']>>): void {
    this.title = s.title;
    this.description = s.description;
    this.rules = s.rules.join('\n');
    this.terms = s.terms;
    this.originalTerms = s.terms;
    this.customized.set(s.terms_customized);
    this.effective.set(s.terms_effective_date);
  }

  protected async save(): Promise<void> {
    this.error.set('');
    this.saved.set(false);
    this.busy.set(true);
    try {
      const rules = this.rules
        .split('\n')
        .map((r) => r.trim())
        .filter((r) => r);
      // Unchanged generated terms are not sent, so they stay generated.
      const terms = this.terms === this.originalTerms ? undefined : this.terms.trim();
      this.apply(
        await this.household.updateServer({
          title: this.title.trim(),
          description: this.description.trim(),
          rules,
          ...(terms === undefined ? {} : { terms }),
        }),
      );
      this.saved.set(true);
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
