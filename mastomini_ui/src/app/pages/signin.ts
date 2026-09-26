import { Component, inject, signal } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';

import { describe } from '../api/api';
import { Auth } from '../api/auth';

@Component({
  selector: 'app-signin',
  imports: [RouterLink],
  template: `
    <h1>Household app</h1>
    <p class="lede">
      Your signed-in devices and password, and for admins, members, invite links and server
      settings. Posting, following and your profile live in your Mastodon app.
    </p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (auth.me()) {
      <p>
        You're signed in as <strong>{{ auth.me()?.username }}</strong
        >.
      </p>
      @if (auth.isAdmin() && needsAdmin) {
        <p>Authorize this browser to manage reports and announcements.</p>
        <button class="btn" type="button" [disabled]="busy()" (click)="signIn()">
          Authorize administration
        </button>
      }
      <a class="btn" routerLink="/me">My account</a>
    } @else {
      <button class="btn" type="button" [disabled]="busy()" (click)="signIn()">Sign in</button>
      <p class="muted small">
        New here? Ask an admin for an invite link. Setting up a phone?
        <a routerLink="/connect">Start here</a>.
      </p>
    }
  `,
})
export class SignInPage {
  protected readonly auth = inject(Auth);
  private readonly route = inject(ActivatedRoute);
  protected readonly error = signal('');
  protected readonly busy = signal(false);
  protected get needsAdmin(): boolean {
    return this.route.snapshot.queryParamMap.get('admin') === '1';
  }

  protected async signIn(): Promise<void> {
    this.error.set('');
    this.busy.set(true);
    const next = this.route.snapshot.queryParamMap.get('next');
    // Only paths inside this app, never another origin.
    const returnTo = next?.startsWith('/') && !next.startsWith('//') ? next : '/me';
    try {
      await this.auth.signIn(returnTo, this.needsAdmin && this.auth.isAdmin());
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
