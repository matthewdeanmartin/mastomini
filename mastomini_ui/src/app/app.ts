import { Component, OnInit, inject, signal } from '@angular/core';
import { Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';

import { describe } from './api/api';
import { Auth } from './api/auth';

/**
 * The shell. Connect and Trust are open to anyone; everything else needs a
 * sign-in, which the server's own sign-in page does (spec/06
 * "Authentication").
 */
@Component({
  selector: 'app-root',
  imports: [RouterOutlet, RouterLink, RouterLinkActive],
  template: `
    <header class="top">
      <a class="brand" href="/">mastomini</a>
      <nav>
        <a routerLink="/connect" routerLinkActive="on">Set up a device</a>
        <a routerLink="/trust" routerLinkActive="on">Trust</a>
        @if (auth.me()) {
          <a routerLink="/me" routerLinkActive="on">My account</a>
          @if (auth.isAdmin()) {
            <a routerLink="/admin/members" routerLinkActive="on">Members</a>
            <a routerLink="/admin/server" routerLinkActive="on">Server</a>
            <a routerLink="/admin/health" routerLinkActive="on">Health</a>
          }
          @if (auth.isOwner()) {
            <a routerLink="/admin/security" routerLinkActive="on">Security</a>
          }
        }
      </nav>
      @if (auth.me(); as me) {
        <span class="who">
          {{ me.username }}
          <button class="link" type="button" (click)="signOut()">Sign out</button>
        </span>
      } @else if (ready()) {
        <a routerLink="/signin">Sign in</a>
      }
    </header>
    <main class="screen">
      @if (error()) {
        <p class="error">{{ error() }}</p>
      }
      @if (ready()) {
        <router-outlet />
      } @else {
        <p class="muted">Loading…</p>
      }
    </main>
  `,
})
export class App implements OnInit {
  protected readonly auth = inject(Auth);
  private readonly router = inject(Router);
  protected readonly ready = signal(false);
  protected readonly error = signal('');

  async ngOnInit(): Promise<void> {
    try {
      const returnTo = await this.auth.start();
      if (returnTo) await this.router.navigateByUrl(returnTo);
      else this.router.initialNavigation();
    } catch (e) {
      this.error.set(describe(e));
      this.router.initialNavigation();
    }
    this.ready.set(true);
  }

  protected async signOut(): Promise<void> {
    await this.auth.signOut();
    await this.router.navigateByUrl('/signin');
  }
}
