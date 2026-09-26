import { Component, OnDestroy, OnInit, inject, signal } from '@angular/core';

import { describe } from '../api/api';
import { ActivityEvent } from '../api/models';
import { Session } from '../api/session';
import { when } from './format';

/** What the bots have been doing, newest first (the last 200 events, in RAM). */
@Component({
  selector: 'app-activity',
  template: `
    <h1>Activity</h1>
    <p class="lede">The last 200 events since the device started. Updates by itself.</p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    <section class="panel">
      <ul class="rows">
        @for (e of events(); track e.seq) {
          <li>
            <span [class]="e.level === 'error' ? 'error' : e.level === 'ok' ? 'ok' : ''">
              @if (e.bot) {
                <strong>{{ e.bot }}</strong>:
              }
              {{ e.text }}
            </span>
            <div class="muted small">{{ e.at ? when(e.at) : 'before the clock was set' }}</div>
          </li>
        } @empty {
          <li class="muted">Nothing yet.</li>
        }
      </ul>
    </section>
  `,
})
export class ActivityPage implements OnInit, OnDestroy {
  private readonly session = inject(Session);
  protected readonly events = signal<ActivityEvent[]>([]);
  protected readonly error = signal('');
  protected readonly when = when;
  private timer?: ReturnType<typeof setInterval>;

  async ngOnInit(): Promise<void> {
    await this.load();
    this.timer = setInterval(() => void this.load(), 3000);
  }

  ngOnDestroy(): void {
    clearInterval(this.timer);
  }

  /** Only what is new since the newest event shown. */
  private async load(): Promise<void> {
    try {
      const since = this.events()[0]?.seq ?? 0;
      const fresh = await this.session.activity(since, 200);
      if (fresh.length) this.events.set([...fresh, ...this.events()].slice(0, 200));
    } catch (e) {
      this.error.set(describe(e));
    }
  }
}
