import { Component, inject, signal } from '@angular/core';
import { DatePipe } from '@angular/common';
import { Api, describe } from '../api/api';

interface Pending {
  id: string; board: string; action: string; stage: string; code: string;
  not_before_ms: number; expires_ms: number; summary: string; approved_by: string;
  approval_account: string; reply_to: string;
  evidence: string;
}

@Component({
  selector: 'app-sre-actions',
  imports: [DatePipe],
  template: `
    <p><button class="btn btn--quiet" type="button" (click)="load()" [disabled]="busy()">Show pending action</button></p>
    @if (error()) { <p class="error">{{ error() }}</p> }
    @if (loaded()) {
      <p class="small">Actions start off. A restart requires separate board keys, a successful dry run,
        your approval, and both the SRE bot and “Execute approved actions” switched on.</p>
      @if (pending(); as p) {
        <p><strong>{{ p.board }} — {{ p.action }}</strong><br>{{ p.stage }}: {{ p.summary }}</p>
        @if (p.evidence) { <p>{{ p.evidence }}</p> }
        <p>Window: {{ p.not_before_ms | date:'MMM d, HH:mm':'UTC' }}–{{ p.expires_ms | date:'HH:mm':'UTC' }} UTC</p>
        @if (p.stage === 'pending') {
          <p><button class="btn" type="button" (click)="send('approve')" [disabled]="busy()">Approve this restart in this window</button></p>
          @if (p.approval_account && p.reply_to) {
            <p class="small">Or reply from admin account {{ p.approval_account }} to digest {{ p.reply_to }}:
              <code>approve {{ p.code }}</code>. Mention the SRE bot in that reply.</p>
          }
        }
        @if (p.stage === 'pending' || p.stage === 'approved' || p.stage === 'new') {
          <button class="btn btn--quiet" type="button" (click)="send('cancel')" [disabled]="busy()">Cancel action</button>
        }
      }
      <p><button class="btn btn--quiet" type="button" (click)="send('propose')" [disabled]="busy()">Prepare restart for review (dry run only)</button></p>
      <p class="muted small">Refresh to see worker results. An uncertain outcome is never retried automatically.</p>
    }
  `,
})
export class SreActions {
  private readonly api = inject(Api);
  protected readonly pending = signal<Pending | null>(null);
  protected readonly loaded = signal(false);
  protected readonly busy = signal(false);
  protected readonly error = signal('');
  protected async load(): Promise<void> { await this.request(); }
  protected async send(action: string): Promise<void> { await this.request(action); }
  private async request(action?: string): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true); this.error.set('');
    try {
      const path = '/api/v1/bots/sre/actions';
      const p = this.pending();
      const response = action
        ? await this.api.post<{pending: Pending | null}>(path + '/' + action, {id: p?.id, code: p?.code})
        : await this.api.get<{pending: Pending | null}>(path);
      this.pending.set(response.pending); this.loaded.set(true);
    } catch (e) { this.error.set(describe(e)); }
    finally { this.busy.set(false); }
  }
}
