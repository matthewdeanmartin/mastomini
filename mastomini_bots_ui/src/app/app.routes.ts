import { inject } from '@angular/core';
import { CanActivateFn, Router, Routes } from '@angular/router';

import { Session } from './api/session';

const signedIn: CanActivateFn = () =>
  inject(Session).signedIn() || inject(Router).parseUrl('/signin');

export const routes: Routes = [
  {
    path: 'signin',
    loadComponent: () => import('./pages/signin').then((m) => m.SigninPage),
    title: 'Sign in — mastomini-bots',
  },
  {
    path: 'bots',
    canActivate: [signedIn],
    loadComponent: () => import('./pages/bots').then((m) => m.BotsPage),
    title: 'Bots — mastomini-bots',
  },
  {
    path: 'activity',
    canActivate: [signedIn],
    loadComponent: () => import('./pages/activity').then((m) => m.ActivityPage),
    title: 'Activity — mastomini-bots',
  },
  {
    path: 'device',
    canActivate: [signedIn],
    loadComponent: () => import('./pages/device').then((m) => m.DevicePage),
    title: 'Device — mastomini-bots',
  },
  { path: '', pathMatch: 'full', redirectTo: 'bots' },
  { path: '**', redirectTo: 'bots' },
];
