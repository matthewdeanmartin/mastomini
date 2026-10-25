import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';
import { Api, describe } from '../api/api';
import { Auth } from '../api/auth';
import { when } from './format';

/** What the board keeps for each account followed elsewhere. */
export interface Feed {
  url: string;
  last_ok_at: string | null;
  last_try_at: string | null;
  failures: number;
  error: string | null;
  gone: boolean;
  posts: number;
}

export interface RemoteAccount {
  id: string;
  acct: string;
  display_name: string;
  url: string;
  avatar: string;
  feed?: Feed;
}

export interface RemoteView {
  enabled: boolean;
  search_server: string;
  feeds_max_board: number;
  feeds_max_account: number;
  images: boolean;
  board_count: number;
  following: RemoteAccount[];
}

/** One line about a feed's last night. */
export function feedStatus(feed: Feed | undefined): string {
  if (!feed) return '';
  if (feed.gone) return 'The account or its feed is gone.';
  if (feed.error) return `Last try failed: ${feed.error}`;
  if (!feed.last_ok_at) return 'Posts appear after the next nightly fetch.';
  return `${feed.posts} posts kept. Fetched ${when(feed.last_ok_at)}.`;
}

/** Accounts on other servers only: local ones have no `@` in `acct`. */
export function remoteOnly(accounts: RemoteAccount[]): RemoteAccount[] {
  return accounts.filter((a) => a.acct.includes('@'));
}

@Component({
  selector: 'app-remote',
  imports: [FormsModule, RouterLink],
  template: `
    <h1>Accounts elsewhere</h1>
    <p class="lede">
      Follow people on other Mastodon servers. Their public posts arrive in your home timeline
      once a night, in every app. This board never sends anything to them.
    </p>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }
    @if (view(); as v) {
      @if (!v.enabled) {
        <p class="muted">Following accounts on other servers is turned off for this household.</p>
      } @else {
        <form class="panel" (ngSubmit)="search()">
          <label for="remote-q">Find a person</label>
          <input
            id="remote-q"
            name="q"
            type="search"
            autocomplete="off"
            placeholder="@name@server or a name"
            [(ngModel)]="query"
          />
          <button class="btn" type="submit" [disabled]="busy() || query.trim().length < 2">
            Search
          </button>
          <p class="small muted">
            @if (v.search_server) {
              Names are looked up on {{ v.search_server }}; a full &#64;name&#64;server asks that
              server.
            } @else {
              Type a full &#64;name&#64;server.
            }
          </p>
        </form>
        @if (searched() && !results().length) {
          <p>No accounts found.</p>
        }
        <ul class="rows">
          @for (a of results(); track a.id) {
            <li>
              <a [href]="a.url" target="_blank" rel="noopener noreferrer">{{
                a.display_name || a.acct
              }}</a>
              <span class="muted"> &#64;{{ a.acct }}</span>
              @if (following(a.id)) {
                <button class="btn btn--quiet" [disabled]="busy()" (click)="unfollow(a)">
                  Unfollow
                </button>
              } @else {
                <button class="btn" [disabled]="busy()" (click)="follow(a)">Follow</button>
              }
            </li>
          }
        </ul>
      }
      <h2>Following ({{ v.following.length }} of {{ v.feeds_max_account }})</h2>
      <p class="small muted">
        The household follows {{ v.board_count }} of at most {{ v.feeds_max_board }} accounts
        elsewhere.
        {{
          v.images
            ? 'Small copies of their pictures are kept on the SD card.'
            : 'No SD card: posts come without pictures.'
        }}
      </p>
      @if (!v.following.length) {
        <p>Nobody yet.</p>
      }
      <ul class="rows">
        @for (a of v.following; track a.id) {
          <li>
            <a [href]="a.url" target="_blank" rel="noopener noreferrer">{{
              a.display_name || a.acct
            }}</a>
            <span class="muted"> &#64;{{ a.acct }}</span>
            <p class="small" [class.error]="a.feed?.error || a.feed?.gone">{{ status(a.feed) }}</p>
            <button class="btn btn--quiet" [disabled]="busy()" (click)="unfollow(a)">
              Unfollow
            </button>
          </li>
        }
      </ul>
      @if (auth.isAdmin()) {
        <p><a routerLink="/admin/remote">Limits, search server and the bots board's key</a></p>
      }
    } @else if (!error()) {
      <p role="status">Loading…</p>
    }
  `,
})
export class RemotePage implements OnInit {
  private readonly api = inject(Api);
  readonly auth = inject(Auth);
  readonly view = signal<RemoteView | null>(null);
  readonly results = signal<RemoteAccount[]>([]);
  readonly searched = signal(false);
  readonly busy = signal(false);
  readonly error = signal('');
  readonly status = feedStatus;
  query = '';

  async ngOnInit(): Promise<void> {
    await this.run(() => this.load());
  }

  following(id: string): boolean {
    return this.view()?.following.some((a) => a.id === id) ?? false;
  }

  async search(): Promise<void> {
    const q = this.query.trim();
    await this.run(async () => {
      const found = await this.api.get<{ accounts: RemoteAccount[] }>(
        `/api/v2/search?type=accounts&resolve=true&limit=10&q=${encodeURIComponent(q)}`,
      );
      this.results.set(remoteOnly(found.accounts));
      this.searched.set(true);
    });
  }

  async follow(a: RemoteAccount): Promise<void> {
    await this.run(async () => {
      await this.api.post(`/api/v1/accounts/${encodeURIComponent(a.id)}/follow`);
      await this.load();
    });
  }

  async unfollow(a: RemoteAccount): Promise<void> {
    await this.run(async () => {
      await this.api.post(`/api/v1/accounts/${encodeURIComponent(a.id)}/unfollow`);
      await this.load();
    });
  }

  private async load(): Promise<void> {
    this.view.set(await this.api.get<RemoteView>('/api/mastomini/v1/remote'));
  }

  private async run(action: () => Promise<void>): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    try {
      await action();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
