import { Component, OnInit, inject, signal } from '@angular/core';
import { Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';

import { describe } from './api/api';
import { Session } from './api/session';

/** The shell: one admin, three screens. */
@Component({
  selector: 'app-root',
  imports: [RouterOutlet, RouterLink, RouterLinkActive],
  template: `
    <header class="top">
      <a class="brand" routerLink="/bots">mastomini-bots</a>
      @if (session.signedIn()) {
        <nav>
          <a routerLink="/bots" routerLinkActive="on">Bots</a>
          <a routerLink="/activity" routerLinkActive="on">Activity</a>
          <a routerLink="/device" routerLinkActive="on">Device</a>
        </nav>
        <span class="who">
          admin
          <button class="link" type="button" (click)="signOut()">Sign out</button>
        </span>
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
  protected readonly session = inject(Session);
  private readonly router = inject(Router);
  protected readonly ready = signal(false);
  protected readonly error = signal('');

  async ngOnInit(): Promise<void> {
    try {
      const signedIn = await this.session.start();
      this.router.initialNavigation();
      if (!signedIn) await this.router.navigateByUrl('/signin');
    } catch (e) {
      this.error.set(describe(e));
      this.router.initialNavigation();
    }
    this.ready.set(true);
  }

  protected async signOut(): Promise<void> {
    await this.session.logout();
    await this.router.navigateByUrl('/signin');
  }
}
