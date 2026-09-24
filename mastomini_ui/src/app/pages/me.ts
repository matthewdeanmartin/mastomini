import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { describe } from '../api/api';
import { Auth } from '../api/auth';
import { Household } from '../api/household';
import { Device } from '../api/models';
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
      one.
    </p>
    <button class="btn btn--danger" type="button" (click)="signOutEverywhere()">
      Sign out everywhere
    </button>

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
        At least 4 characters. Every device is signed out; sign in again with the new password.
      </p>
      <button class="btn" type="submit" [disabled]="busy()">Change password</button>
    </form>
  `,
})
export class MePage implements OnInit {
  protected readonly auth = inject(Auth);
  private readonly household = inject(Household);
  protected readonly devices = signal<Device[]>([]);
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
      this.devices.set(await this.household.devices());
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
