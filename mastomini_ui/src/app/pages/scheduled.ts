import { Component, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { Api, describe } from '../api/api';

interface ScheduledPost {
  id: string;
  scheduled_at: string;
  params: { text: string; visibility: string };
  mastomini_delivery: string;
  mastomini_error: string | null;
}

@Component({
  selector: 'app-scheduled',
  imports: [FormsModule],
  template: `
    <h1>Scheduled posts</h1>
    <p class="lede">
      Choose a time at least five minutes ahead. Times below use your device’s time zone. If either
      board is offline, the post will publish as soon as they can reconnect.
    </p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (saved()) {
      <p class="ok">{{ saved() }}</p>
    }
    <form class="panel" (ngSubmit)="schedule()">
      <label for="post">Post</label>
      <textarea id="post" name="post" rows="4" [(ngModel)]="text" required></textarea>
      <label for="warning">Content warning (optional)</label>
      <input id="warning" name="warning" [(ngModel)]="warning" />
      <label for="visibility">Audience</label>
      <select id="visibility" name="visibility" [(ngModel)]="visibility">
        <option value="public">Public</option>
        <option value="unlisted">Unlisted</option>
        <option value="private">Followers only</option>
      </select>
      <label for="at">Publish at</label>
      <input id="at" name="at" type="datetime-local" [(ngModel)]="at" required />
      <button class="btn" type="submit" [disabled]="busy()">Schedule post</button>
    </form>
    <button class="btn" type="button" (click)="reload()" [disabled]="busy()">
      Refresh scheduled posts
    </button>
    @for (post of posts(); track post.id) {
      <section class="panel">
        <p style="white-space: pre-wrap">{{ post.params.text }}</p>
        <p class="muted">{{ status(post) }}</p>
        @if (post.mastomini_error) {
          <p class="error">{{ post.mastomini_error }}</p>
        }
        <label [for]="'at-' + post.id">Publish at</label>
        <input [id]="'at-' + post.id" type="datetime-local" [(ngModel)]="dates[post.id]" />
        <button
          class="btn"
          type="button"
          (click)="reschedule(post)"
          [disabled]="busy() || !!post.mastomini_error"
        >
          Change time
        </button>
        <button class="btn" type="button" (click)="cancel(post)" [disabled]="busy()">
          Cancel post
        </button>
      </section>
    } @empty {
      <p class="muted">No scheduled posts.</p>
    }
  `,
})
export class ScheduledPage implements OnInit {
  private readonly api = inject(Api);
  protected readonly posts = signal<ScheduledPost[]>([]);
  protected readonly busy = signal(false);
  protected readonly error = signal('');
  protected readonly saved = signal('');
  protected dates: Record<string, string> = {};
  protected text = '';
  protected warning = '';
  protected visibility = 'public';
  protected at = '';
  private submission: { body: string; key: string } | null = null;

  ngOnInit(): void {
    void this.reload();
  }

  protected status(post: ScheduledPost): string {
    return post.mastomini_delivery === 'pending_handoff'
      ? 'Saved; waiting for the bots board.'
      : post.mastomini_delivery === 'failed'
        ? 'Could not publish.'
        : 'Scheduled on the bots board.';
  }

  private async load(): Promise<void> {
    const posts = await this.api.get<ScheduledPost[]>('/api/v1/scheduled_statuses');
    this.posts.set(posts);
    this.dates = Object.fromEntries(
      posts.map((p) => {
        const date = new Date(p.scheduled_at);
        return [
          p.id,
          new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16),
        ];
      }),
    );
  }

  private async run(action: () => Promise<void>): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    this.saved.set('');
    try {
      await action();
    } catch (e) {
      this.error.set(e instanceof RangeError ? 'Choose a valid date and time.' : describe(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected reload(): Promise<void> {
    return this.run(() => this.load());
  }

  protected schedule(): Promise<void> {
    return this.run(async () => {
      const body = {
        status: this.text,
        spoiler_text: this.warning,
        visibility: this.visibility,
        scheduled_at: new Date(this.at).toISOString(),
      };
      const fingerprint = JSON.stringify(body);
      if (this.submission?.body !== fingerprint) {
        const bytes = crypto.getRandomValues(new Uint8Array(16));
        this.submission = {
          body: fingerprint,
          key: Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join(''),
        };
      }
      await this.api.post('/api/v1/statuses', body, this.submission.key);
      this.submission = null;
      this.text = '';
      this.warning = '';
      this.saved.set('Post saved for scheduling.');
      await this.load();
    });
  }

  protected reschedule(post: ScheduledPost): Promise<void> {
    return this.run(async () => {
      await this.api.put('/api/v1/scheduled_statuses/' + post.id, {
        scheduled_at: new Date(this.dates[post.id]).toISOString(),
      });
      await this.load();
      this.saved.set('Publication time updated.');
    });
  }

  protected cancel(post: ScheduledPost): Promise<void> {
    return this.run(async () => {
      await this.api.delete('/api/v1/scheduled_statuses/' + post.id);
      await this.load();
      this.saved.set('Scheduled post cancelled.');
    });
  }
}
