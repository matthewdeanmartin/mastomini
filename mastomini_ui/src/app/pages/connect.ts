import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { RouterLink } from '@angular/router';

import { Household } from '../api/household';
import { Status } from '../api/models';
import { copyText } from './clipboard';
import { DeviceKind, GUIDES, NEEDS_TEXT, detectDevice, works } from './connect-guide';

/** Getting a device into a real Mastodon app (spec/06 "Client setup guide"). */
@Component({
  selector: 'app-connect',
  imports: [RouterLink],
  template: `
    <h1>Set up a device</h1>
    <p class="lede">Use a Mastodon app for posting and reading. Here's how to connect one.</p>

    <section class="panel">
      <h2 style="margin-top: 0">Server address</h2>
      <p class="link-box">{{ address }}</p>
      <button class="btn" type="button" (click)="copy()">Copy</button>
      @if (copied()) {
        <span class="ok">Copied.</span>
      }
      @if (status()?.host; as host) {
        @if (host !== address) {
          <p class="muted small">
            The server's own name is <strong>{{ host }}</strong>. The address above is the one this
            browser used, and it works from any device on the home Wi-Fi.
          </p>
        }
      }
    </section>

    @if (status() && !status()!.https) {
      <section class="panel">
        <strong>This server uses plain HTTP for now.</strong>
        <p class="muted small">
          Phone apps haven't been tested with it yet, and many require HTTPS, so they may refuse
          to connect until the owner sets HTTPS up. See <a routerLink="/trust">Trust</a>.
        </p>
      </section>
    }

    <h2>Steps</h2>
    <ol>
      <li>Install one of the apps below.</li>
      <li>Choose to sign in to a server (sometimes "Log in" or "Other server"), and enter the
        address above.</li>
      <li>The app opens this server's sign-in page. Enter your username and password, then
        <strong>Approve</strong>.</li>
    </ol>
    <p class="muted small">
      No account yet? Ask an admin for an invite link.
    </p>

    <h2>Apps</h2>
    <p>
      @for (g of guides; track g.kind) {
        <button class="btn" [class.btn--quiet]="g.kind !== device()" type="button"
                (click)="device.set(g.kind)">{{ g.label }}</button>
      }
    </p>
    @if (guide(); as g) {
      <section class="panel">
        <ul class="rows">
          @for (c of g.clients; track c.name) {
            <li>
              <strong>{{ c.name }}</strong>
              @if (c.tested) {
                <span class="tag">tested</span>
              }
              @if (status() && works(c, status()!.https)) {
                <span class="tag">should work now</span>
              }
              <div class="muted small">
                {{ needs(c.needs) }}.
                @if (!c.tested) {
                  Not tested with mastomini yet.
                }
                {{ c.note ?? '' }}
              </div>
            </li>
          }
        </ul>
        @for (n of g.notes; track n) {
          <p class="muted small">{{ n }}</p>
        }
      </section>
    }

    <h2>Good to know</h2>
    <ul>
      <li>Ask whoever runs the router to give the board a fixed address (a "DHCP
        reservation"), so the address above doesn't change.</li>
      <li>Don't run two boards with the same name on one network.</li>
      <li>Signing in on a device is what unlocks your direct messages on it.</li>
    </ul>
  `,
})
export class ConnectPage implements OnInit {
  private readonly household = inject(Household);
  protected readonly guides = GUIDES;
  protected readonly address = location.host;
  protected readonly status = signal<Status | null>(null);
  protected readonly device = signal<DeviceKind>(detectDevice(navigator.userAgent));
  protected readonly guide = computed(() => GUIDES.find((g) => g.kind === this.device()));
  protected readonly copied = signal(false);
  protected readonly works = works;

  async ngOnInit(): Promise<void> {
    this.status.set(await this.household.status().catch(() => null));
  }

  protected needs(needs: (keyof typeof NEEDS_TEXT)[]): string {
    const text = needs.map((n) => NEEDS_TEXT[n]).join(', or ');
    return text.charAt(0).toUpperCase() + text.slice(1);
  }

  protected async copy(): Promise<void> {
    await copyText(this.address);
    this.copied.set(true);
  }
}
