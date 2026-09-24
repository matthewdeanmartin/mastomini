import { Component, OnInit, inject, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { Household } from '../api/household';
import { Status } from '../api/models';
import { DeviceKind, detectDevice } from './connect-guide';

/** Installing the household certificate, per device (spec/05 Model A). */
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
      'Most apps still ignore it: Android apps need the real-domain setup.',
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
      'Copy it to /usr/local/share/ca-certificates/ (as .crt) and run sudo update-ca-certificates.',
      'Firefox keeps its own list: Settings → Certificates → View Certificates → Authorities → Import.',
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
          <strong>Nothing to install yet.</strong>
          <p class="muted small">
            This server uses plain HTTP, so there is no certificate to trust. Anyone on the home
            Wi-Fi could read what passes between your device and the board, so treat the Wi-Fi
            password as the key to the house.
          </p>
          <p class="muted small">
            HTTPS is planned. When the owner turns it on, this page will offer the household
            certificate and walk each device through trusting it. iPhones and Macs will use that
            certificate; most Android apps will need the server on a real domain name instead.
          </p>
        </section>
      } @else {
        <p class="lede">
          Install the household certificate once on each device, so apps can check they are
          talking to this board.
        </p>
        <a class="btn" href="/ca" download>Download the certificate</a>
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
