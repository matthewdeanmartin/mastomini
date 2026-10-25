import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute } from '@angular/router';
import { Api, describe } from '../api/api';
import { when } from './format';
import { Feed, feedStatus } from './remote';

interface Followed {
  id: string;
  acct: string;
  display_name: string;
  followers: string[];
  feed: Feed;
}

interface DeviceKey {
  id: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

export interface RemoteAdmin {
  enabled: boolean;
  search_server: string;
  feeds_max_board: number;
  feeds_max_account: number;
  feeds_ceiling_board: number;
  feeds_ceiling_account: number;
  media_quota_mb: number;
  images: boolean;
  media_bytes: number;
  board_count: number;
  /** Who the household searches as, when signed in to the search server. */
  search_account: string | null;
  search_signed_in: boolean;
  accounts: Followed[];
  device_keys: DeviceKey[];
}

const PATH = '/api/mastomini/v1/admin/remote';
const LOGIN = '/api/mastomini/v1/admin/search_login';

/** Large general servers with working search; any other can be typed. */
export const SEARCH_SERVERS = [
  'mastodon.social',
  'mastodon.online',
  'mastodon.nu',
  'mstdn.social',
  'mas.to',
  'fosstodon.org',
  'hachyderm.io',
];

/** The admin-page message for `?search=…` after a sign-in comes back. */
export function signInMessage(search: string | null, reason: string | null): [string, string] {
  if (search === 'connected') return ['Signed in. Searches now find posts and hashtags too.', ''];
  if (search === 'failed') return ['', `Signing in did not work: ${reason || 'unknown reason'}.`];
  return ['', ''];
}

@Component({
  selector: 'app-remote-admin',
  imports: [FormsModule],
  template: `
    <h1>Accounts elsewhere</h1>
    <p class="lede">
      Members follow accounts on other servers by their RSS feeds. The bots board fetches the
      feeds at night with a device key and hands the posts to this board.
    </p>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }
    @if (notice()) {
      <p class="ok" role="status">{{ notice() }}</p>
    }
    @if (view(); as v) {
      <section class="panel">
        <h2>Search server</h2>
        <p class="small muted">
          Searches for names go to this server. Signed in, they also find posts and #hashtags,
          through that account with read-only access. Its results are not filtered.
        </p>
        @if (v.search_account) {
          <p class="ok">
            Signed in to {{ v.search_server }} as &#64;{{ v.search_account }}. Everyone in the
            household searches through this account.
          </p>
          <button class="btn btn--quiet" type="button" [disabled]="busy()" (click)="signOut()">
            Sign out
          </button>
        } @else if (v.search_server) {
          <p>Using {{ v.search_server }} without signing in: only account names are searched.</p>
        } @else {
          <p>No search server: only full &#64;name&#64;server addresses are looked up.</p>
        }
        <fieldset [disabled]="busy()">
          <label for="server-choice">Server</label>
          <select id="server-choice" name="choice" [(ngModel)]="choice">
            @for (name of servers; track name) {
              <option [value]="name">{{ name }}</option>
            }
            <option value="other">Another server…</option>
            <option value="">None</option>
          </select>
          @if (choice === 'other') {
            <label for="server-other">Server name</label>
            <input id="server-other" name="other" placeholder="example.social" [(ngModel)]="other" />
          }
          <button class="btn" type="button" [disabled]="!chosen()" (click)="signIn()">
            Sign in to {{ chosen() || 'the server' }}
          </button>
          <button class="btn btn--quiet" type="button" (click)="useServer()">
            Use without signing in
          </button>
          <p class="small muted">
            Signing in opens the server's own login page; this board never sees the password.
          </p>
        </fieldset>
      </section>

      <form class="panel" (ngSubmit)="save()">
        <h2>Settings</h2>
        <fieldset [disabled]="busy()">
          <label
            ><input name="enabled" type="checkbox" [(ngModel)]="form.enabled" /> Members may search
            and follow accounts on other servers</label
          >
          <label for="remote-board"
            >Most accounts for the household (0–{{ v.feeds_ceiling_board }})</label
          >
          <input
            id="remote-board"
            name="board"
            type="number"
            min="0"
            [max]="v.feeds_ceiling_board"
            [(ngModel)]="form.feeds_max_board"
          />
          <label for="remote-account"
            >Most accounts per person (0–{{ v.feeds_ceiling_account }})</label
          >
          <input
            id="remote-account"
            name="account"
            type="number"
            min="0"
            [max]="v.feeds_ceiling_account"
            [(ngModel)]="form.feeds_max_account"
          />
          <label for="remote-quota">Most MiB of pictures on the SD card</label>
          <input
            id="remote-quota"
            name="quota"
            type="number"
            min="0"
            [(ngModel)]="form.media_quota_mb"
          />
          <p class="small muted">
            {{
              v.images
                ? 'SD card: ' + mib(v.media_bytes) + ' MiB of pictures kept.'
                : 'No SD card: posts come without pictures.'
            }}
            Lowering a limit never unfollows anyone; it stops new follows.
          </p>
          <button class="btn" type="submit">Save</button>
        </fieldset>
      </form>

      <h2>Followed accounts ({{ v.board_count }} of {{ v.feeds_max_board }})</h2>
      @if (!v.accounts.length) {
        <p>None yet.</p>
      }
      <ul class="rows">
        @for (a of v.accounts; track a.id) {
          <li>
            <strong>{{ a.display_name || a.acct }}</strong>
            <span class="muted"> &#64;{{ a.acct }}</span>
            <p class="small">Followed by {{ a.followers.join(', ') }}.</p>
            <p class="small" [class.error]="a.feed.error || a.feed.gone">{{ status(a.feed) }}</p>
          </li>
        }
      </ul>

      <h2>Device keys</h2>
      <p class="small muted">
        The bots board's Feeds bot uses one: its server is this board's address and its API key the
        device key. A device key can only read the list of feeds and hand in posts.
      </p>
      @if (newKey()) {
        <p class="ok" role="status">
          Copy this key into the Feeds bot now; it is not shown again:
          <code>{{ newKey() }}</code>
        </p>
      }
      <ul class="rows">
        @for (k of v.device_keys; track k.id) {
          <li>
            <strong>{{ k.name }}</strong>
            <span class="small muted">
              Made {{ when(k.created_at) }}.
              {{
                k.last_used_at
                  ? 'Last used ' + when(k.last_used_at) + '.'
                  : 'Not used since the board started.'
              }}
            </span>
            <button class="btn btn--danger" [disabled]="busy()" (click)="revoke(k)">Revoke</button>
          </li>
        }
      </ul>
      <form (ngSubmit)="create()">
        <label for="key-name">New key's name</label>
        <input id="key-name" name="keyName" placeholder="Bots board" [(ngModel)]="keyName" />
        <button class="btn" type="submit" [disabled]="busy() || !keyName.trim()">
          Make a device key
        </button>
      </form>
    } @else if (!error()) {
      <p role="status">Loading…</p>
    }
  `,
})
export class RemoteAdminPage implements OnInit {
  private readonly api = inject(Api);
  private readonly route = inject(ActivatedRoute);
  /** Where the browser goes to sign in (replaced in tests). */
  navigate = (url: string) => location.assign(url);
  readonly servers = SEARCH_SERVERS;
  choice = 'mastodon.social';
  other = '';
  readonly view = signal<RemoteAdmin | null>(null);
  readonly busy = signal(false);
  readonly error = signal('');
  readonly notice = signal('');
  readonly newKey = signal('');
  readonly status = feedStatus;
  readonly when = when;
  form = {
    enabled: true,
    feeds_max_board: 30,
    feeds_max_account: 10,
    media_quota_mb: 256,
  };
  keyName = 'Bots board';

  async ngOnInit(): Promise<void> {
    await this.run(async () => this.show(await this.api.get<RemoteAdmin>(PATH)));
    const query = this.route.snapshot.queryParamMap;
    const [notice, error] = signInMessage(query.get('search'), query.get('reason'));
    if (notice) this.notice.set(notice);
    if (error) this.error.set(error);
  }

  /** The chosen server's name, or '' for none. */
  chosen(): string {
    return (this.choice === 'other' ? this.other : this.choice).trim().toLowerCase();
  }

  async signIn(): Promise<void> {
    await this.run(async () => {
      const r = await this.api.post<{ authorize_url: string }>(LOGIN, { server: this.chosen() });
      this.navigate(r.authorize_url);
    });
  }

  async useServer(): Promise<void> {
    await this.run(async () => {
      this.show(await this.api.put<RemoteAdmin>(PATH, { search_server: this.chosen() }));
      this.notice.set('Saved.');
    });
  }

  async signOut(): Promise<void> {
    await this.run(async () => {
      await this.api.delete(LOGIN);
      this.show(await this.api.get<RemoteAdmin>(PATH));
      this.notice.set('Signed out: only account names are searched now.');
    });
  }

  mib(bytes: number): string {
    return (bytes / 1048576).toFixed(1);
  }

  async save(): Promise<void> {
    await this.run(async () => {
      this.show(await this.api.put<RemoteAdmin>(PATH, { ...this.form }));
      this.notice.set('Saved.');
    });
  }

  async create(): Promise<void> {
    await this.run(async () => {
      const made = await this.api.post<{ key: string }>('/api/mastomini/v1/admin/device_keys', {
        name: this.keyName.trim(),
      });
      this.newKey.set(made.key);
      this.show(await this.api.get<RemoteAdmin>(PATH));
    });
  }

  async revoke(k: DeviceKey): Promise<void> {
    if (!confirm(`Revoke "${k.name}"? Its feeds stop until it gets a new key.`)) return;
    await this.run(async () => {
      await this.api.delete(`/api/mastomini/v1/admin/device_keys/${encodeURIComponent(k.id)}`);
      this.show(await this.api.get<RemoteAdmin>(PATH));
    });
  }

  private show(v: RemoteAdmin): void {
    this.view.set(v);
    if (!v.search_server) this.choice = '';
    else if (SEARCH_SERVERS.includes(v.search_server)) this.choice = v.search_server;
    else {
      this.choice = 'other';
      this.other = v.search_server;
    }
    this.form = {
      enabled: v.enabled,
      feeds_max_board: v.feeds_max_board,
      feeds_max_account: v.feeds_max_account,
      media_quota_mb: v.media_quota_mb,
    };
  }

  private async run(action: () => Promise<void>): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    this.notice.set('');
    try {
      await action();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
