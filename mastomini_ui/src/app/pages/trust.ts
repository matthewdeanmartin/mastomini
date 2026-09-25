import { Component, OnInit, inject, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { Household } from '../api/household';
import { Status } from '../api/models';
import { DeviceKind, detectDevice } from './connect-guide';

/**
 * Installing the household certificate, per device (spec/05 Model A). Kept in
 * step with the server's own /trust page (mastomini_rs/src/api/trust.rs),
 * which works without the app.
 */
const STEPS: { kind: DeviceKind; label: string; steps: string[] }[] = [
  {
    kind: 'ios',
    label: 'iPhone / iPad',
    steps: [
      'Open this page in Safari and tap "Download the certificate", then Allow.',
      'Settings → General → VPN & Device Management → the mastomini profile → Install.',
      'Settings → General → About → Certificate Trust Settings → turn on full trust for it.',
    ],
  },
  {
    kind: 'android',
    label: 'Android',
    steps: [
      'Download the certificate.',
      'Settings → Security → Encryption & credentials → Install a certificate → CA certificate.',
      'Chrome then trusts the board, but most Android apps ignore it: they need the real-domain setup.',
    ],
  },
  {
    kind: 'mac',
    label: 'Mac',
    steps: [
      'Download the certificate and open it: Keychain Access adds it.',
      'Double-click it in Keychain Access → Trust → "When using this certificate: Always Trust".',
    ],
  },
  {
    kind: 'windows',
    label: 'Windows',
    steps: [
      'Download the certificate and open it → Install Certificate → Current User.',
      'Choose "Place all certificates in the following store" → Trusted Root Certification Authorities.',
    ],
  },
  {
    kind: 'linux',
    label: 'Linux',
    steps: [
      'Download the PEM version from /ca.pem, copy it to /usr/local/share/ca-certificates/mastomini.crt and run sudo update-ca-certificates.',
      'Firefox keeps its own list: Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import.',
    ],
  },
];

@Component({
  selector: 'app-trust',
  imports: [RouterLink],
  template: `
    <h1>Trust this server</h1>
    @if (status(); as s) {
      @if (!s.https) {
        <section class="panel">
          <strong>Nothing to install.</strong>
          <p class="muted small">
            This server uses plain HTTP only, so there is no certificate to trust. Anyone on the
            home Wi-Fi could read what passes between your device and the board, so treat the Wi-Fi
            password as the key to the house.
          </p>
          <p class="muted small">
            The owner turns HTTPS on by building the firmware with a household certificate (make
            certs). Then this page offers it and walks each device through trusting it.
          </p>
        </section>
      } @else {
        @if (s.secure) {
          <section class="panel">
            <strong class="ok">This device already trusts the board.</strong>
            <p class="muted small">You're connected over HTTPS. Use this page for other devices.</p>
          </section>
        }
        <p class="lede">
          Install the household certificate once on each device. Then apps can check they are
          talking to this board, and nobody else on the Wi-Fi can read or change what you send.
        </p>
        <a class="btn" href="/ca" download>Download the certificate</a>
        <p class="muted small">
          Before trusting it, compare its SHA-256 fingerprint with the one the owner has:
        </p>
        <p class="link-box">{{ s.ca_fingerprint }}</p>
        <p>
          @for (g of steps; track g.kind) {
            <button class="btn" [class.btn--quiet]="g.kind !== device()" type="button"
                    (click)="device.set(g.kind)">{{ g.label }}</button>
          }
        </p>
        @for (g of steps; track g.kind) {
          @if (g.kind === device()) {
            <ol class="panel">
              @for (step of g.steps; track step) {
                <li>{{ step }}</li>
              }
            </ol>
          }
        }
        @if (!s.secure && s.https_url) {
          <p>
            Then use <a [href]="s.https_url + '/app/'">{{ s.https_url }}</a>. If it shows a
            warning, the certificate isn't trusted yet: don't click past it.
          </p>
        }
      }
    } @else {
      <p class="muted">Loading…</p>
    }
    <p class="muted small">
      Don't run two boards with the same name on one network: devices can't tell them apart.
      Next: <a routerLink="/connect">set up a device</a>.
    </p>
  `,
})
export class TrustPage implements OnInit {
  private readonly household = inject(Household);
  protected readonly status = signal<Status | null>(null);
  protected readonly steps = STEPS;
  protected readonly device = signal<DeviceKind>(detectDevice(navigator.userAgent));

  async ngOnInit(): Promise<void> {
    this.status.set(await this.household.status().catch(() => null));
  }
}
