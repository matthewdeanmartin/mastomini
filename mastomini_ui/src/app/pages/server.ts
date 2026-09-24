import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { describe } from '../api/api';
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
  `,
})
export class ServerPage implements OnInit {
  private readonly household = inject(Household);
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
