import { Component, OnDestroy, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';
import { Api, describe } from '../api/api';

export interface SetupBot {
  id: string; username: string; display_name: string; strategy: string; initial_grant: boolean;
}
interface Progress {
  running: boolean;
  rows: { id: string; username: string; state: string }[];
  error: string | null;
  plan?: { nanacoin: string; mastomini: string; bots: SetupBot[] } | null;
}
export const strategies = [
  ['band', 'Forex band'], ['lotto', 'Lotto'], ['borrow', 'Borrower'], ['lend', 'Bot bank'],
  ['dca', 'Steady buyer'], ['rebalance', 'Rebalancer'], ['revert', 'Mean reversion'],
];
export function defaults(): SetupBot[] {
  return ['nana_news', ...Array.from({ length: 6 }, (_, i) => `trader_${i + 1}`)].map((id, i) => ({
    id, username: id, display_name: i === 0 ? 'Nana news' : `Trader ${i}`,
    strategy: i === 0 ? '' : strategies[i - 1][0], initial_grant: false,
  }));
}

@Component({
  selector: 'app-nanacoin-setup',
  imports: [FormsModule, RouterLink],
  template: `
    <h1>Set up NanaCoin bots</h1>
    <p class="lede">Sign in once to each server. This device creates the accounts and saves their keys for you.</p>
    <p><a routerLink="/bots">← Bots</a></p>
    @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    <form (ngSubmit)="start()">
      <fieldset [disabled]="busy()">
        <section class="panel">
          <h2>Connect the servers</h2>
          <label>NanaCoin server <input name="nanacoin" [(ngModel)]="nanacoin" required /></label>
          <label>Nana username <input name="nanaUsername" [(ngModel)]="nanaUsername" autocomplete="username" required /></label>
          <label>Nana password <input name="nanaPassword" type="password" [(ngModel)]="nanaPassword" autocomplete="off" required /></label>
          <label>Mastomini server <input name="mastomini" [(ngModel)]="mastomini" required /></label>
          <label>Mastomini admin username <input name="adminUsername" [(ngModel)]="adminUsername" autocomplete="username" required /></label>
          <label>Mastomini admin password <input name="adminPassword" type="password" [(ngModel)]="adminPassword" autocomplete="off" required /></label>
          <p class="muted small">Admin credentials are used for this setup only. Use HTTPS, or localhost for development.</p>
        </section>
        <section class="panel">
          <h2>Review the accounts</h2>
          <p>New bots start off. Traders start in dry-run. Connected bots keep their existing settings.</p>
          @for (bot of bots; track bot.id) {
            <div class="panel">
              <label><input type="checkbox" [name]="bot.id + 'selected'" [(ngModel)]="selected[bot.id]" /> {{ bot.id }}</label>
              @if (selected[bot.id]) {
                <label>Username <input [name]="bot.id + 'username'" [(ngModel)]="bot.username" pattern="[a-z0-9_]{1,24}" required /></label>
                <label>Display name <input [name]="bot.id + 'display'" [(ngModel)]="bot.display_name" maxlength="40" required /></label>
                @if (bot.id !== 'nana_news') {
                  <label>Strategy <select [name]="bot.id + 'strategy'" [(ngModel)]="bot.strategy">
                    @for (strategy of strategies; track strategy[0]) { <option [value]="strategy[0]">{{ strategy[1] }}</option> }
                  </select></label>
                  <label><input type="checkbox" [name]="bot.id + 'grant'" [(ngModel)]="bot.initial_grant" /> Give Nana's configured initial grant when creating this account</label>
                } @else {
                  <p class="muted small">Uses Nana's read-only key; no NanaCoin trading account is created.</p>
                }
              }
            </div>
          }
          <label><input name="replaceRead" type="checkbox" [(ngModel)]="replaceRead" /> Allow replacing Nana's existing read-only key for the news bot</label>
          <p class="muted small">Leave this unchecked to preserve an existing read key. Other keys are preserved.</p>
          <button class="btn" type="submit" [disabled]="!hasSelection()">Create and connect</button>
        </section>
      </fieldset>
    </form>
    @if (progress(); as progress) {
      <section class="panel" aria-live="polite">
        <h2>{{ progress.running ? 'Connecting…' : progress.error ? 'Setup needs attention' : 'Setup progress' }}</h2>
        @for (row of progress.rows; track row.id) { <p>{{ row.username }} — {{ row.state }}</p> }
        @if (progress.error) { <p class="error">{{ progress.error }}</p><p>Enter the admin passwords again and resume with the same servers and usernames. Completed accounts and grants will be kept.</p> }
        @if (!progress.running && progress.rows.length && !progress.error) { <p class="ok">Accounts connected. Review the bots before enabling them or turning off dry-run.</p> }
      </section>
    }
  `,
})
export class NanaCoinSetupPage implements OnInit, OnDestroy {
  private readonly api = inject(Api);
  readonly progress = signal<Progress | null>(null);
  readonly error = signal('');
  readonly busy = signal(false);
  readonly strategies = strategies;
  bots = defaults();
  selected: Record<string, boolean> = Object.fromEntries(this.bots.map(b => [b.id, true]));
  nanacoin = 'https://nanacoin.local'; mastomini = 'https://mastomini.local';
  nanaUsername = 'nana'; adminUsername = ''; nanaPassword = ''; adminPassword = '';
  replaceRead = false;
  private timer?: ReturnType<typeof setInterval>;
  private polling = false;
  private destroyed = false;
  async ngOnInit(): Promise<void> {
    try {
      const progress = await this.api.get<Progress>('/api/v1/integrations/nanacoin/setup');
      if (this.destroyed) return;
      if (progress.plan) {
        this.nanacoin = progress.plan.nanacoin; this.mastomini = progress.plan.mastomini;
        this.selected = Object.fromEntries(this.bots.map(b => [b.id, false]));
        for (const row of progress.plan.bots) {
          const i = this.bots.findIndex(b => b.id === row.id);
          if (i >= 0) { this.bots[i] = row; this.selected[row.id] = true; }
        }
      }
      this.setProgress(progress);
    } catch (error) { this.error.set(describe(error)); }
  }
  ngOnDestroy(): void { this.destroyed = true; clearInterval(this.timer); this.clearPasswords(); }
  hasSelection(): boolean { return this.bots.some(b => this.selected[b.id]); }
  private clearPasswords(): void { this.nanaPassword = ''; this.adminPassword = ''; }
  async start(): Promise<void> {
    if (this.busy() || !this.hasSelection()) return;
    this.busy.set(true); this.error.set('');
    try {
      const request = this.api.post<Progress>('/api/v1/integrations/nanacoin/setup', {
        nanacoin: this.nanacoin, mastomini: this.mastomini,
        nana_username: this.nanaUsername, nana_password: this.nanaPassword,
        admin_username: this.adminUsername, admin_password: this.adminPassword,
        replace_news_read_key: this.replaceRead, bots: this.bots.filter(b => this.selected[b.id]),
      });
      this.clearPasswords();
      this.setProgress(await request);
    } catch (error) { this.error.set(describe(error)); this.busy.set(false); }
  }
  private setProgress(progress: Progress): void {
    if (this.destroyed) return;
    this.progress.set(progress); this.busy.set(progress.running);
    if (progress.running && !this.timer) this.timer = setInterval(() => void this.poll(), 1000);
    if (!progress.running) { clearInterval(this.timer); this.timer = undefined; }
  }
  private async poll(): Promise<void> {
    if (this.polling) return;
    this.polling = true;
    try { this.setProgress(await this.api.get<Progress>('/api/v1/integrations/nanacoin/setup')); }
    catch (error) { this.error.set(describe(error)); }
    finally { this.polling = false; }
  }
}
