import {
  ApplicationConfig,
  provideBrowserGlobalErrorListeners,
  provideZonelessChangeDetection,
} from '@angular/core';
import { provideRouter, withDisabledInitialNavigation, withHashLocation } from '@angular/router';

import { routes } from './app.routes';

export const appConfig: ApplicationConfig = {
  providers: [
    provideBrowserGlobalErrorListeners(),
    // Signals throughout, so zone.js has nothing to do.
    provideZonelessChangeDetection(),
    // Hash routing: the board serves one index.html at /app/ and needs no
    // list of client routes to fall back from.
    // The first navigation waits for the session (App.ngOnInit), so a
    // reloaded admin page isn't bounced by its guard before we know who is
    // signed in.
    provideRouter(routes, withHashLocation(), withDisabledInitialNavigation()),
  ],
};
