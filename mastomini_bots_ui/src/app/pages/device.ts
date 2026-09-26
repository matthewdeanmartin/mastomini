import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';

import { describe } from '../api/api';
import { Diag, OpenRouterStatus } from '../api/models';
import { Session } from '../api/session';
import { bytes, duration, when } from './format';

/** The board itself: memory, Wi-Fi, clock, build, HTTPS, the admin password. */
@Component({
  selector: 'app-device',
  imports: [FormsModule],
  template: `
    <h1>Device</h1>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (diag(); as d) {
      <section class="panel">
        <dl class="facts">
          <dt>Running</dt>
          <dd>{{ d.build.name }} {{ d.build.version }}
            ({{ d.build.commit || 'no commit' }}{{ d.build.dirty ? ', uncommitted changes' : '' }}),
            built {{ when(d.build.built_at) }}</dd>
          <dt>Up for</dt>
          <dd>{{ duration(d.platform.uptime_ms) }} on {{ d.platform.target }}
            @if (d.platform.reset_reason) {
              (last restart: {{ d.platform.reset_reason }})
            }</dd>
          <dt>Clock</dt>
          <dd>{{ d.clock ? when(d.clock) : 'Not set yet: bots wait for internet time' }}</dd>
          @if (d.platform.wifi_rssi !== null) {
            <dt>Wi-Fi</dt>
            <dd>{{ d.platform.wifi_rssi }} dBm{{ d.platform.ip ? ' · ' + d.platform.ip : '' }}</dd>
          }
          @if (d.platform.heap_internal_free !== null) {
            <dt>Memory</dt>
            <dd>{{ bytes(d.platform.heap_internal_free) }} internal free
              (lowest {{ bytes(d.platform.heap_internal_min_free) }}),
              {{ bytes(d.platform.psram_free) }} PSRAM free</dd>
          }
        </dl>
      </section>
    }
    @if (session.status()?.https; as https) {
      <h2>HTTPS</h2>
      <p class="muted small">
        Certificate for {{ https.names.join(', ') }}, from {{ https.ca_name }}, valid until
        {{ https.not_after.slice(0, 10) }}. Devices that trust mastomini already trust this; others
        can <a href="/ca">download the household CA</a>.
      </p>
    }
    <h2>OpenRouter</h2>
    <p class="muted small">
      Bots that write with a language model use this one key.
      @if (openRouter(); as o) {
        Used by: {{ o.used_by.join(', ') || 'no bot' }}.
      }
      Make a key at openrouter.ai (Keys), with a credit limit: every model call is billed.
    </p>
    @if (openRouter(); as o) {
      <form class="panel" (ngSubmit)="saveOpenRouter()">
        <label for="or-key">API key</label>
        <input id="or-key" name="orKey" type="password" autocomplete="off" [(ngModel)]="orKey"
               [placeholder]="o.key_set ? 'Saved — type a new one to replace it' : 'sk-or-…'" />
        <label for="or-model">Default model</label>
        <input id="or-model" name="orModel" type="text" autocapitalize="none" [(ngModel)]="orModel"
               [placeholder]="o.default_model" />
        <p class="muted small">
          An OpenRouter model id. Empty: {{ o.default_model }}. A bot's own Model setting overrides it.
        </p>
        <button class="btn" type="submit" [disabled]="busy()">Save</button>
        @if (o.key_set) {
          <button class="btn btn--danger" type="button" (click)="removeOpenRouterKey()">Remove key</button>
        }
        @if (saved()) {
          <span class="ok small"> Saved.</span>
        }
      </form>
    }
    <h2>Admin password</h2>
    <form class="panel" (ngSubmit)="changePassword()">
      <label for="current">Current password</label>
      <input id="current" name="current" type="password" autocomplete="current-password"
             [(ngModel)]="current" required />
      <label for="next">New password</label>
      <input id="next" name="next" type="password" autocomplete="new-password" [(ngModel)]="next" required />
      <p class="muted small">At least 8 characters. Every session, this one included, is signed out.</p>
      <button class="btn" type="submit" [disabled]="busy()">Change password</button>
    </form>
  `,
})
export class DevicePage implements OnInit {
  protected readonly session = inject(Session);
  private readonly router = inject(Router);
  protected readonly diag = signal<Diag | null>(null);
  protected readonly openRouter = signal<OpenRouterStatus | null>(null);
  protected readonly saved = signal(false);
  protected orKey = '';
  protected orModel = '';
  protected readonly error = signal('');
  protected readonly busy = signal(false);
  protected readonly when = when;
  protected readonly bytes = bytes;
  protected readonly duration = duration;
  protected current = '';
  protected next = '';

  async ngOnInit(): Promise<void> {
    try {
      this.diag.set(await this.session.diag());
      const o = await this.session.openRouter();
      this.openRouter.set(o);
      this.orModel = o.model;
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async saveOpenRouter(): Promise<void> {
    this.error.set('');
    this.saved.set(false);
    this.busy.set(true);
    try {
      const update = { model: this.orModel, ...(this.orKey ? { key: this.orKey } : {}) };
      this.openRouter.set(await this.session.setOpenRouter(update));
      this.orKey = '';
      this.saved.set(true);
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected async removeOpenRouterKey(): Promise<void> {
    if (!confirm('Remove the OpenRouter key? Bots that use a model stop working.')) return;
    try {
      this.openRouter.set(await this.session.setOpenRouter({ key: '' }));
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async changePassword(): Promise<void> {
    this.error.set('');
    this.busy.set(true);
    try {
      await this.session.changePassword(this.current, this.next);
      await this.session.logout().catch(() => undefined);
      await this.router.navigateByUrl('/signin');
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
