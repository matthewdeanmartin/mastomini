import { Component, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Router } from '@angular/router';

import { describe } from '../api/api';
import { Session } from '../api/session';

/** First visit: choose the admin password. After that: sign in. */
@Component({
  selector: 'app-signin',
  imports: [FormsModule],
  template: `
    @if (setup()) {
      <h1>Set up mastomini-bots</h1>
      <p class="lede">
        Choose the admin password. Whoever knows it can change what the bots post and where, so
        make it a good one: at least 8 characters.
      </p>
    } @else {
      <h1>Sign in</h1>
    }
    @if (insecure(); as https) {
      <p class="panel small">
        This page is plain HTTP, so the password crosses the network unencrypted.
        <a [href]="https">Use HTTPS</a> once this device trusts the household CA (the same one
        mastomini uses; <a href="/ca">download it</a>).
      </p>
    }
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    <form class="panel" (ngSubmit)="submit()">
      <label for="password">{{ setup() ? 'New admin password' : 'Admin password' }}</label>
      <input id="password" name="password" type="password" [(ngModel)]="password" required
             [attr.autocomplete]="setup() ? 'new-password' : 'current-password'" />
      @if (setup()) {
        <label for="again">Again</label>
        <input id="again" name="again" type="password" autocomplete="new-password"
               [(ngModel)]="again" required />
      }
      <button class="btn" type="submit" [disabled]="busy()">
        {{ setup() ? 'Set password' : 'Sign in' }}
      </button>
    </form>
  `,
})
export class SigninPage {
  private readonly session = inject(Session);
  private readonly router = inject(Router);
  protected readonly error = signal('');
  protected readonly busy = signal(false);
  protected readonly setup = computed(() => this.session.status()?.setup_needed ?? false);
  /** An HTTPS address for this page, when this page is HTTP and HTTPS exists. */
  protected readonly insecure = computed(() => {
    const status = this.session.status();
    if (!status || status.secure || !status.https) return null;
    return `https://${location.hostname}/app/`;
  });
  protected password = '';
  protected again = '';

  protected async submit(): Promise<void> {
    this.error.set('');
    if (this.setup() && this.password !== this.again) {
      this.error.set('The two passwords are different.');
      return;
    }
    this.busy.set(true);
    try {
      if (this.setup()) await this.session.setup(this.password);
      else await this.session.login(this.password);
      await this.session.start();
      await this.router.navigateByUrl('/bots');
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
