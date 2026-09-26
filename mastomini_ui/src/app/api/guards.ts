// The server enforces every rule regardless; these only keep the app from
// offering screens whose every button would answer 403.

import { inject } from '@angular/core';
import { CanActivateFn, CanDeactivateFn, Router } from '@angular/router';

import { Auth } from './auth';

/** Signed-out visitors go to the sign-in page, which comes back here after. */
export const signedIn: CanActivateFn = (_route, state) =>
  inject(Auth).me() !== null ||
  inject(Router).createUrlTree(['/signin'], { queryParams: { next: state.url } });

export const adminOnly: CanActivateFn = (route, state) => {
  const auth = inject(Auth);
  if (!auth.me()) return signedIn(route, state);
  return auth.isAdmin() || inject(Router).createUrlTree(['/me']);
};

export const ownerOnly: CanActivateFn = (route, state) => {
  const auth = inject(Auth);
  if (!auth.me()) return signedIn(route, state);
  return auth.isOwner() || inject(Router).createUrlTree(['/me']);
};

/** Existing household routes keep their role-based contract; moderation needs OAuth admin scopes. */
export const scopedAdminOnly: CanActivateFn = (route, state) => {
  const auth = inject(Auth);
  if (!auth.me() || !auth.isAdmin()) return adminOnly(route, state);
  return (
    auth.canAdminister() ||
    inject(Router).createUrlTree(['/signin'], {
      queryParams: { next: state.url, admin: '1' },
    })
  );
};

export const unsavedAdminChanges: CanDeactivateFn<{ canLeave(): boolean }> = (page) =>
  !inject(Auth).me() || page.canLeave();
