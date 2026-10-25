import { inject } from '@angular/core';
import { CanActivateFn, Router, Routes } from '@angular/router';

import { Session } from './api/session';

const signedIn: CanActivateFn = () =>
  inject(Session).signedIn() || inject(Router).parseUrl('/signin');

export const routes: Routes = [
  { path: 'nanacoin-setup', canActivate: [signedIn], loadComponent: () => import('./pages/nanacoin-setup').then(m => m.NanaCoinSetupPage), title: 'Set up NanaCoin bots — mastomini-bots' },
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
    path: 'sre',
    canActivate: [signedIn],
    loadComponent: () => import('./pages/sre').then((m) => m.SrePage),
    title: 'SRE — mastomini-bots',
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
