import { Component, OnInit, inject, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { describe } from '../api/api';
import { Household } from '../api/household';
import { Security } from '../api/models';

/**
 * The owner's view of how the household is protected (spec/06
 * `/admin/security`). HTTPS, the Easy/Secure switch and certificates arrive
 * with Sprint 8; until then this shows the transport as it is.
 */
@Component({
  selector: 'app-security',
  imports: [RouterLink],
  template: `
    <h1>Security</h1>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (s(); as s) {
      <h2>Connection</h2>
      <section class="panel">
        @if (!s.transport.https) {
          <p><strong>Plain HTTP only.</strong> Passwords and posts cross the home Wi-Fi
            unencrypted, and many phone apps won't connect.</p>
          <p class="muted small">
            HTTPS is planned: a household certificate each device trusts once (see
            <a routerLink="/trust">Trust</a>), or a real domain name with a public certificate for
            Android. Then you'll be able to switch to Secure mode here, which turns plain HTTP off
            except for the pages new devices need.
          </p>
        } @else {
          <p>Mode: {{ s.transport.mode }}</p>
        }
      </section>

      <h2>Direct messages</h2>
      <section class="panel">
        @if (withoutKeys(s).length) {
          <p>
            Waiting for a sign-in: <strong>{{ withoutKeys(s).join(', ') }}</strong>. Nobody can send
            them direct messages until they sign in again, which creates their encryption key.
          </p>
        } @else {
          <p>Every member has an encryption key. Direct messages are readable only by the people
            in them, not by admins or the owner.</p>
        }
      </section>

      <h2>Members and devices</h2>
      <section class="panel">
        <ul class="rows">
          @for (m of s.members; track m.username) {
            <li>
              <strong>{{ m.username }}</strong>
              @if (m.role !== 'member') {
                <span class="tag">{{ m.role }}</span>
              }
              <span class="muted">· {{ m.devices }} of {{ s.tokens_per_member }} devices</span>
            </li>
          }
        </ul>
        <p class="muted small">
          {{ s.tokens[0] }} of {{ s.tokens[1] }} sign-ins and {{ s.apps[0] }} of {{ s.apps[1] }}
          registered apps in use. A member's oldest device is signed out when they sign in on a
          ninth. Members manage their own devices under My account.
        </p>
      </section>

      <h2>Passwords</h2>
      <section class="panel">
        <p>
          At least {{ s.passwords.min_length }} characters. {{ s.passwords.lockout_failures }}
          wrong tries lock a username for {{ s.passwords.lockout_minutes }} minutes.
        </p>
        <p class="muted small">
          Short passwords are allowed because being on the home Wi-Fi is the first lock. But
          someone holding the board itself can guess a short password offline, and with it read
          that member's direct messages. Encourage longer ones.
        </p>
      </section>
    } @else if (!error()) {
      <p class="muted">Loading…</p>
    }
  `,
})
export class SecurityPage implements OnInit {
  private readonly household = inject(Household);
  protected readonly s = signal<Security | null>(null);
  protected readonly error = signal('');

  async ngOnInit(): Promise<void> {
    try {
      this.s.set(await this.household.security());
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected withoutKeys(s: Security): string[] {
    return s.members.filter((m) => !m.message_key).map((m) => m.username);
  }
}
