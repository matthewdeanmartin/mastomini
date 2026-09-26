import { Routes } from '@angular/router';

import { adminOnly, ownerOnly, signedIn, scopedAdminOnly, unsavedAdminChanges } from './api/guards';

/**
 * Lazy, so members never download the admin screens. Connect and Trust are
 * public: they are for devices that have no account signed in yet.
 */
export const routes: Routes = [
  {
    path: 'connect',
    loadComponent: () => import('./pages/connect').then((m) => m.ConnectPage),
    title: 'Set up a device — mastomini',
  },
  {
    path: 'trust',
    loadComponent: () => import('./pages/trust').then((m) => m.TrustPage),
    title: 'Trust this server — mastomini',
  },
  {
    path: 'signin',
    loadComponent: () => import('./pages/signin').then((m) => m.SignInPage),
    title: 'Sign in — mastomini',
  },
  {
    path: 'me',
    loadComponent: () => import('./pages/me').then((m) => m.MePage),
    title: 'My account — mastomini',
    canActivate: [signedIn],
  },
  {
    path: 'admin/members',
    loadComponent: () => import('./pages/members').then((m) => m.MembersPage),
    title: 'Members — mastomini',
    canActivate: [adminOnly],
  },
  {
    path: 'admin/moderation',
    loadComponent: () => import('./pages/moderation').then((m) => m.ModerationPage),
    title: 'Moderation — mastomini',
    canActivate: [scopedAdminOnly],
    canDeactivate: [unsavedAdminChanges],
  },
  {
    path: 'admin/announcements',
    loadComponent: () => import('./pages/announcements').then((m) => m.AnnouncementsPage),
    title: 'Announcements — mastomini',
    canActivate: [scopedAdminOnly],
    canDeactivate: [unsavedAdminChanges],
  },
  {
    path: 'admin/server',
    loadComponent: () => import('./pages/server').then((m) => m.ServerPage),
    title: 'Server — mastomini',
    canActivate: [adminOnly],
  },
  {
    path: 'admin/health',
    loadComponent: () => import('./pages/health').then((m) => m.HealthPage),
    title: 'Health — mastomini',
    canActivate: [adminOnly],
  },
  {
    path: 'admin/security',
    loadComponent: () => import('./pages/security').then((m) => m.SecurityPage),
    title: 'Security — mastomini',
    canActivate: [ownerOnly],
  },
  { path: '', pathMatch: 'full', redirectTo: 'me' },
  { path: '**', redirectTo: 'me' },
];
