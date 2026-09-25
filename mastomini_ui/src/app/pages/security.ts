import { Component, OnInit, inject, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { describe } from '../api/api';
import { Household } from '../api/household';
import { Security } from '../api/models';

/**
 * The owner's view of how the household is protected (spec/06
 * `/admin/security`): the transport and certificates (docs/security/https.md),
 * direct-message keys, and members' devices.
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
            This firmware was built without a certificate. On the build computer, run make certs
            and deploy again: the board then serves HTTPS too, with a household certificate each
            device trusts once (see <a routerLink="/trust">Trust</a>).
          </p>
        } @else {
          <p><strong>HTTPS and plain HTTP</strong> (Easy mode): both serve everything.</p>
          @if (s.transport.this_connection === 'http') {
            <p class="error">
              You're using plain HTTP right now. Once this device trusts the household
              certificate, use <a [href]="s.certificate?.https_url + '/app/'">{{ s.certificate?.https_url }}</a>.
            </p>
          }
          <p class="muted small">
            Devices still using http:// addresses send passwords and posts unencrypted. Move each
            device to HTTPS from the <a routerLink="/trust">Trust</a> page. Secure mode, which turns
            plain HTTP off except for the pages new devices need, isn't built yet.
          </p>
          @if (s.household_ca; as ca) {
            <h3>Household certificate authority</h3>
            <p>{{ ca.name }}</p>
            <p class="muted small">SHA-256 fingerprint, to compare on each device:</p>
            <p class="link-box">{{ ca.fingerprint_sha256 }}</p>
            <p class="muted small">
              @if (ca.name_constrained) {
                It can only vouch for household names and private addresses, never for public
                websites.
              } @else {
                It has no name limits: a device that trusts it believes anything it signs.
              }
            </p>
          }
          @if (s.certificate; as cert) {
            <h3>The board's certificate</h3>
            <p>
              For {{ cert.names.join(', ') }}. Expires {{ cert.not_after.slice(0, 10) }}
              ({{ daysLeft(cert.not_after) }} days).
            </p>
            @if (daysLeft(cert.not_after) < 60) {
              <p class="error">
                Renew it soon: on the build computer run make reissue-cert, then deploy. Devices
                keep trusting the same household certificate.
              </p>
            }
          }
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

  protected daysLeft(iso: string): number {
    return Math.floor((Date.parse(iso) - Date.now()) / 86_400_000);
  }

  protected withoutKeys(s: Security): string[] {
    return s.members.filter((m) => !m.message_key).map((m) => m.username);
  }
}
