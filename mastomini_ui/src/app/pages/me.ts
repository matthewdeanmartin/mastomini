import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { describe } from '../api/api';
import { Auth } from '../api/auth';
import { Household } from '../api/household';
import { ApiKey, Device } from '../api/models';
import { copyText } from './clipboard';
import { when } from './format';

/** Devices, password, sign out everywhere: security the Mastodon API lacks. */
@Component({
  selector: 'app-me',
  imports: [FormsModule],
  template: `
    <h1>My account</h1>
    <p class="lede">
      Signed in as <strong>{{ auth.me()?.username }}</strong>. Change your name, bio and profile in
      your Mastodon app.
    </p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }

    <h2>Signed-in devices</h2>
    <section class="panel">
      <ul class="rows">
        @for (d of devices(); track d.id) {
          <li>
            <strong>{{ d.app.name || 'Unknown app' }}</strong>
            @if (d.current) {
              <span class="tag">this browser</span>
            }
            <div class="muted small">
              Signed in {{ when(d.created_at) }}
              @if (d.last_used_at) {
                · last used {{ when(d.last_used_at) }}
              } @else {
                · not used since the server restarted
              }
            </div>
            <button class="btn btn--quiet" type="button" (click)="revoke(d)">Sign out</button>
          </li>
        } @empty {
          <li class="muted">Loading…</li>
        }
      </ul>
    </section>
    <p class="muted small">
      Lost a phone? Sign it out here. Signing out everywhere ends every session, including this
      one, and revokes your API keys.
    </p>
    <button class="btn btn--danger" type="button" (click)="signOutEverywhere()">
      Sign out everywhere
    </button>

    <h2>API keys</h2>
    <p class="muted small">
      For bots and scripts that post as you. Send the key as
      <code>Authorization: Bearer &lt;key&gt;</code> to the Mastodon API, for example
      <code>curl -H "Authorization: Bearer KEY" -d "status=Hello" {{ origin }}/api/v1/statuses</code>.
      Posts show the key's name as the app. A key keeps working until you revoke it, change your
      password or sign out everywhere. It cannot read your direct messages.
    </p>
    @if (newKey(); as k) {
      <section class="panel">
        <p><strong>{{ k.name }}</strong>: copy the key now. It is not shown again.</p>
        <p><code class="secret">{{ k.key }}</code></p>
        <button class="btn" type="button" (click)="copyKey(k)">{{ copied() ? 'Copied' : 'Copy key' }}</button>
        <button class="btn btn--quiet" type="button" (click)="newKey.set(null)">Done</button>
      </section>
    }
    <section class="panel">
      <ul class="rows">
        @for (k of keys(); track k.id) {
          <li>
            <strong>{{ k.name }}</strong>
            @for (s of k.scopes; track s) {
              <span class="tag">{{ s }}</span>
            }
            <div class="muted small">
              Made {{ when(k.created_at) }}
              @if (k.last_used_at) {
                · last used {{ when(k.last_used_at) }}
              }
            </div>
            <button class="btn btn--quiet" type="button" (click)="revokeKey(k)">Revoke</button>
          </li>
        } @empty {
          <li class="muted">No API keys.</li>
        }
      </ul>
    </section>
    <form class="panel" (ngSubmit)="createKey()">
      <label for="key-name">Name</label>
      <input id="key-name" name="keyName" type="text" maxlength="40" placeholder="Weather bot"
             [(ngModel)]="keyName" required />
      <label>
        <input type="checkbox" name="keyWrite" [(ngModel)]="keyWrite" />
        Can post and act (write); otherwise read only
      </label>
      <label for="key-password">Your password</label>
      <input id="key-password" name="keyPassword" type="password" autocomplete="current-password"
             [(ngModel)]="keyPassword" required />
      <button class="btn" type="submit" [disabled]="busy()">Make a key</button>
    </form>

    <h2>Change password</h2>
    <form class="panel" (ngSubmit)="changePassword()">
      <label for="current">Current password</label>
      <input id="current" name="current" type="password" autocomplete="current-password"
             [(ngModel)]="current" required />
      <label for="next">New password</label>
      <input id="next" name="next" type="password" autocomplete="new-password"
             [(ngModel)]="next" required />
      <label for="again">New password again</label>
      <input id="again" name="again" type="password" autocomplete="new-password"
             [(ngModel)]="again" required />
      <p class="muted small">
        At least 4 characters. Every device is signed out and your API keys are revoked; sign in
        again with the new password.
      </p>
      <button class="btn" type="submit" [disabled]="busy()">Change password</button>
    </form>
  `,
})
export class MePage implements OnInit {
  protected readonly auth = inject(Auth);
  private readonly household = inject(Household);
  protected readonly devices = signal<Device[]>([]);
  protected readonly keys = signal<ApiKey[]>([]);
  protected readonly newKey = signal<ApiKey | null>(null);
  protected readonly copied = signal(false);
  protected readonly origin = location.origin;
  protected keyName = '';
  protected keyWrite = true;
  protected keyPassword = '';
  protected readonly error = signal('');
  protected readonly busy = signal(false);
  protected readonly when = when;
  protected current = '';
  protected next = '';
  protected again = '';

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  private async load(): Promise<void> {
    try {
      const [devices, keys] = await Promise.all([
        this.household.devices(),
        this.household.apiKeys(),
      ]);
      this.devices.set(devices);
      this.keys.set(keys);
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async revoke(device: Device): Promise<void> {
    const name = device.app.name || 'this app';
    if (!confirm(device.current ? 'Sign out of this browser?' : `Sign out ${name}?`)) return;
    try {
      await this.household.revokeDevice(device.id);
      if (device.current) this.auth.forget();
      else await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async createKey(): Promise<void> {
    this.error.set('');
    this.busy.set(true);
    try {
      const scopes = this.keyWrite ? 'read write' : 'read';
      const key = await this.household.createApiKey(this.keyName, scopes, this.keyPassword);
      this.copied.set(false);
      this.newKey.set(key);
      this.keyName = '';
      this.keyPassword = '';
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected async copyKey(key: ApiKey): Promise<void> {
    if (!key.key) return;
    await copyText(key.key);
    this.copied.set(true);
  }

  protected async revokeKey(key: ApiKey): Promise<void> {
    if (!confirm(`Revoke ${key.name}? Anything using it stops working.`)) return;
    try {
      await this.household.revokeApiKey(key.id);
      if (this.newKey()?.id === key.id) this.newKey.set(null);
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async signOutEverywhere(): Promise<void> {
    if (!confirm('Sign out every device, including this one?')) return;
    try {
      await this.household.signOutEverywhere();
      this.auth.forget();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async changePassword(): Promise<void> {
    this.error.set('');
    if (this.next !== this.again) {
      this.error.set('The two new passwords are different.');
      return;
    }
    this.busy.set(true);
    try {
      await this.household.changePassword(this.current, this.next);
      alert('Password changed. Sign in again with the new password.');
      this.auth.forget();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
