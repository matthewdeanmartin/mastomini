import { Component, OnDestroy, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';

import { describe } from '../api/api';
import { Bot, OpenRouterStatus } from '../api/models';
import { Session } from '../api/session';
import { when } from './format';
import { SettingFields, changedValues, initialValues } from './setting-fields';

interface Draft {
  instance: string;
  token: string;
  enabled: boolean;
}

/** Every bot: what it does, when it runs next, how the last run went. */
@Component({
  selector: 'app-bots',
  imports: [FormsModule, RouterLink, SettingFields],
  template: `
    <h1>Bots</h1>
    <p class="lede">
      Bots are part of this device's firmware. Give each one a Mastodon server and an API key
      (on mastomini: <em>My account → API keys</em>), then turn it on.
    </p>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @for (bot of bots(); track bot.id) {
      <section class="panel">
        <h2>
          {{ bot.name }}
          @if (bot.enabled) {
            <span class="tag">on</span>
          } @else {
            <span class="tag tag--warn">off</span>
          }
          @if (bot.running) {
            <span class="tag">running…</span>
          } @else if (bot.queued) {
            <span class="tag">starting…</span>
          }
          @if (bot.retrying) {
            <span class="tag tag--warn">retrying</span>
          }
          @if (bot.uses_llm) {
            <span class="tag">language model</span>
          }
          @if (usesNanaCoin(bot)) {
            <span class="tag">NanaCoin</span>
          }
        </h2>
        <p class="muted small">{{ bot.description }}</p>
        @if (bot.uses_llm && openRouter() && !openRouter()!.key_set) {
          <p class="error small">
            This bot writes with a language model: add an OpenRouter API key on the
            <a routerLink="/device">Device</a> page first.
          </p>
        }
        @if (isReply(bot)) {
          <p class="small">
            The schedule checks Mastodon for new mentions. No new mentions means no OpenRouter calls.
            A model call is made only for an eligible, unanswered mention; failed attempts may be retried.
            The first check starts watching from now and skips older mentions.
          </p>
        }
        <dl class="facts">
          <dt>{{ isReply(bot) ? 'Check for mentions' : 'Schedule' }}</dt>
          <dd>{{ bot.schedule }}</dd>
          <dt>{{ isReply(bot) ? 'Next mention check' : 'Next run' }}</dt>
          <dd>{{ bot.enabled ? when(bot.next_run_at) : 'Off' }}</dd>
          <dt>Server</dt>
          <dd>
            {{ bot.instance || '—' }} · {{ bot.key_set ? 'API key saved' : 'no API key' }}
            @if (!bot.needs_mastodon && !bot.key_set) {
              <span class="muted small">(optional: it posts only with a key)</span>
            }
          </dd>
          <dt>{{ isReply(bot) ? 'Last mention check' : 'Last run' }}</dt>
          <dd>
            @if (bot.last_run; as run) {
              <span [class]="run.ok ? 'ok' : 'error'">{{ run.ok ? '✓' : '✗' }} {{ run.summary }}</span>
              <span class="muted small"> — {{ when(run.finished_at) }}{{ run.manual ? ', by hand' : '' }}</span>
            } @else {
              Never
            }
          </dd>
          @if (bot.last_check; as check) {
            <dt>Connection check</dt>
            <dd>
              <span [class]="check.ok ? 'ok' : 'error'">{{ check.ok ? '✓' : '✗' }} {{ check.summary }}</span>
              <span class="muted small"> — {{ when(check.finished_at) }}</span>
            </dd>
          }
          <dt>{{ isReply(bot) ? 'Checks' : 'Runs' }}</dt>
          <dd>{{ bot.runs }} ({{ bot.failures }} failed)</dd>
        </dl>
        <button class="btn" type="button" (click)="act(bot, 'run')"
                [disabled]="!bot.ready || bot.running || bot.queued">{{ isReply(bot) ? 'Check mentions now' : 'Run now' }}</button>
        <button class="btn btn--quiet" type="button" (click)="act(bot, 'check')"
                [disabled]="!bot.ready || bot.running || bot.queued">
          {{ usesNanaCoin(bot) ? 'Check connections' : 'Check Mastodon key' }}
        </button>
        @if (bot.uses_llm) {
          <button class="btn btn--quiet" type="button" (click)="act(bot, 'check-model')"
                  [disabled]="!bot.key_set || !openRouter()?.key_set || bot.running || bot.queued">
            Test OpenRouter (1 model request)
          </button>
        }
        <button class="btn btn--quiet" type="button" (click)="edit(bot)">
          {{ editing() === bot.id ? 'Close settings' : 'Settings' }}
        </button>
        @if (editing() === bot.id) {
          <form (ngSubmit)="save(bot)">
            <label [for]="bot.id + '-instance'">Mastodon server{{ bot.needs_mastodon ? '' : ' (optional)' }}</label>
            <input [id]="bot.id + '-instance'" name="instance" type="text" [(ngModel)]="draft.instance"
                   placeholder="https://mastomini.local" autocapitalize="none" />
            <label [for]="bot.id + '-token'">Mastodon API key{{ bot.needs_mastodon ? '' : ' (optional)' }}</label>
            <input [id]="bot.id + '-token'" name="token" type="password" [(ngModel)]="draft.token"
                   autocomplete="off"
                   [placeholder]="bot.key_set ? 'Saved — type a new one to replace it' : 'Paste the key'" />
            <label>
              <input type="checkbox" name="enabled" [(ngModel)]="draft.enabled" />
              On: run on its schedule
            </label>
            @if (bot.settings.length) {
              <h3>{{ bot.name }} settings</h3>
              <app-setting-fields [fields]="bot.settings" [prefix]="bot.id" [(values)]="values" />
            }
            <p class="muted small">
              Turning a bot on runs it from its next scheduled time; use Run now to post straight
              away. It may run up to {{ bot.grace_minutes }} minutes late (after a power cut, or
              while retrying), never later.
            </p>
            <button class="btn" type="submit" [disabled]="busy()">Save</button>
            @if (bot.key_set) {
              <button class="btn btn--danger" type="button" (click)="removeKey(bot)">Remove key</button>
            }
          </form>
        }
      </section>
    } @empty {
      <p class="muted">Loading…</p>
    }
  `,
})
export class BotsPage implements OnInit, OnDestroy {
  private readonly session = inject(Session);
  protected readonly bots = signal<Bot[]>([]);
  protected readonly error = signal('');
  protected readonly busy = signal(false);
  protected readonly editing = signal<string | null>(null);
  protected readonly when = when;
  protected draft: Draft = { instance: '', token: '', enabled: false };
  /** The open form's bot settings, by key. */
  protected readonly values = signal<Record<string, string>>({});
  protected readonly openRouter = signal<OpenRouterStatus | null>(null);
  private timer?: ReturnType<typeof setInterval>;
  private destroyed = false;
  private loading = false;

  async ngOnInit(): Promise<void> {
    await this.load();
    if (this.destroyed) return;
    this.session.openRouter().then((s) => this.openRouter.set(s), () => undefined);
    // Runs take seconds: keep the page current without a reload.
    this.timer = setInterval(() => { if (!document.hidden) void this.load(); }, 10_000);
  }

  ngOnDestroy(): void {
    this.destroyed = true;
    clearInterval(this.timer);
  }

  private async load(): Promise<void> {
    if (this.destroyed || this.loading) return;
    this.loading = true;
    try {
      this.bots.set(await this.session.bots());
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.loading = false;
    }
  }

  /** Has NanaCoin settings: the news and trader bots. */
  protected usesNanaCoin(bot: Bot): boolean {
    return bot.settings.some((field) => field.key === 'nc_key');
  }

  protected isReply(bot: Bot): boolean {
    const mode = bot.settings.find((field) => field.key === 'mode');
    return bot.uses_llm && (mode?.value ?? mode?.default) === 'reply';
  }

  protected edit(bot: Bot): void {
    if (this.editing() === bot.id) {
      this.editing.set(null);
      return;
    }
    this.draft = { instance: bot.instance, token: '', enabled: bot.enabled };
    this.values.set(initialValues(bot.settings));
    this.editing.set(bot.id);
  }

  protected async save(bot: Bot): Promise<void> {
    this.error.set('');
    this.busy.set(true);
    try {
      const update = {
        instance: this.draft.instance,
        enabled: this.draft.enabled,
        ...(this.draft.token ? { token: this.draft.token } : {}),
        settings: changedValues(bot.settings, this.values()),
      };
      await this.session.update(bot.id, update);
      this.editing.set(null);
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected async removeKey(bot: Bot): Promise<void> {
    if (!confirm(`Remove ${bot.name}'s API key? The bot turns off.`)) return;
    try {
      await this.session.update(bot.id, { token: '', enabled: false });
      this.editing.set(null);
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected async act(bot: Bot, action: 'run' | 'check' | 'check-model'): Promise<void> {
    this.error.set('');
    try {
      await this.session.request(bot.id, action);
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }
}
