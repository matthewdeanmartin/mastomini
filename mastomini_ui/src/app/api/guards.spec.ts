import { TestBed } from '@angular/core/testing';
import {
  ActivatedRouteSnapshot,
  Router,
  RouterStateSnapshot,
  provideRouter,
} from '@angular/router';
import { afterEach, describe, expect, it } from 'vitest';
import { Auth } from './auth';
import { scopedAdminOnly } from './guards';

afterEach(() => TestBed.resetTestingModule());
describe('admin route authorization', () => {
  for (const [signedIn, admin, scoped, expected] of [
    [false, false, false, '/signin?next=%2Fadmin%2Fmoderation'],
    [true, false, false, '/me'],
    [true, true, false, '/signin?next=%2Fadmin%2Fmoderation&admin=1'],
    [true, true, true, true],
  ] as const) {
    it(`routes signedIn=${signedIn}, admin=${admin}, scoped=${scoped}`, () => {
      TestBed.configureTestingModule({
        providers: [
          provideRouter([]),
          {
            provide: Auth,
            useValue: {
              me: () => (signedIn ? { id: '1' } : null),
              isAdmin: () => admin,
              canAdminister: () => scoped,
            },
          },
        ],
      });
      const result = TestBed.runInInjectionContext(() =>
        scopedAdminOnly(
          {} as ActivatedRouteSnapshot,
          { url: '/admin/moderation' } as RouterStateSnapshot,
        ),
      );
      expect(result === true ? true : TestBed.inject(Router).serializeUrl(result as never)).toBe(
        expected,
      );
    });
  }
});
