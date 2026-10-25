import { Component, OnInit, inject, signal } from '@angular/core';
import { DatePipe } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';
import { Api, describe } from '../api/api';
import { Session } from '../api/session';
import { Bot } from '../api/models';
import { SettingFields, changedValues, initialValues } from './setting-fields';
import { SreHistory } from './sre-history';
import { SreActions } from './sre-actions';

export interface Overview {
  boards: {
    host: string; url: string; status: string | null; boot_ms: number | null;
    resets: { at_ms: number; kind: string }[] | null;
    heap_kib: number | null; heap_usual_kib: number | null;
    largest_kib: number | null; largest_usual_kib: number | null;
    rssi: number | null; rssi_usual: number | null; readings: number;
  }[];
  findings: { host: string; rule: string; since_ms: number; action: string; todo: string }[];
  observations: string[];
  rules: { id: string; default: string; action: string; todo: string }[];
  notify: { channels: string[]; log: string[] };
  record: { bytes: number; limit: number };
}

type Tab = 'fleet' | 'findings' | 'runbooks' | 'notify' | 'history' | 'setup';

/** `rule = action` lines for every rule that differs from its default. */
export function overrides(rules: Overview['rules'], chosen: Record<string, string>): string {
  return rules
    .filter((r) => (chosen[r.id] ?? r.action) !== r.default)
    .map((r) => `${r.id} = ${chosen[r.id] ?? r.action}`)
    .join('\n');
}

@Component({
  selector: 'app-sre',
  imports: [DatePipe, FormsModule, RouterLink, SettingFields, SreHistory, SreActions],
  template: `
    <h1>SRE</h1>
    <nav class="tabs" aria-label="SRE sections">
      @for (t of tabs; track t[0]) {
        <button type="button" class="btn btn--quiet" [class.on]="tab() === t[0]" (click)="tab.set(t[0])">{{ t[1] }}</button>
      }
    </nav>
    @if (error()) { <p class="error">{{ error() }}</p> }
    @if (o(); as o) {
      @if (o.record.bytes > o.record.limit * 0.85) {
        <p class="error">The SRE bot's saved record is {{ o.record.bytes }} of {{ o.record.limit }} bytes.
          Near the limit it forgets its largest memory (see Activity).</p>
      }
      @switch (tab()) {
        @case ('fleet') {
          <p class="muted small">From the last digest; no board is called. “Usual” is the median of this board's last readings.</p>
          <div style="overflow-x:auto"><table>
            <thead><tr><th scope="col">Board</th><th scope="col">Status</th><th scope="col">Last reset</th>
              <th scope="col">Free RAM</th><th scope="col">Largest block</th><th scope="col">Wi-Fi</th></tr></thead>
            <tbody>
              @for (b of o.boards; track b.host) {
                <tr>
                  <th scope="row"><a [href]="b.url">{{ b.host }}</a></th>
                  <td>{{ b.status ?? 'not seen yet' }}</td>
                  <td>
                    @if (b.resets?.length) { {{ b.resets![0].kind }}, {{ b.resets![0].at_ms | date:'MMM d, HH:mm' }}
                      @if (b.resets!.length > 1) { <span class="muted small">({{ b.resets!.length }} this week)</span> }
                    } @else if (b.boot_ms) { up since {{ b.boot_ms | date:'MMM d' }} } @else { — }
                  </td>
                  <td>{{ reading(b.heap_kib, b.heap_usual_kib, ' KiB', b.readings) }}</td>
                  <td>{{ reading(b.largest_kib, b.largest_usual_kib, ' KiB', b.readings) }}</td>
                  <td>{{ reading(b.rssi, b.rssi_usual, ' dBm', b.readings) }}</td>
                </tr>
              }
            </tbody>
          </table></div>
          <p><a [href]="'/'" (click)="$event.preventDefault(); check()">Check connections now</a>
            <span class="muted small"> (reads every board and sends a test notice)</span></p>
          @if (bot()?.last_check; as c) { <p class="small">{{ c.ok ? '✓' : '✗' }} {{ c.summary }}</p> }
        }
        @case ('findings') {
          @for (f of o.findings; track f.host + f.rule) {
            <p><strong>{{ f.host }}</strong> — {{ f.rule }} <span class="muted small">({{ f.action }}, since {{ f.since_ms | date:'MMM d, HH:mm' }})</span><br>
              {{ f.todo }} <a [href]="boardUrl(o, f.host) + '/.well-known/incidents'">Error log</a></p>
          } @empty { <p>No open findings. Resets are one-off events: see Fleet.</p> }
          @for (line of o.observations; track $index) { <p class="small">{{ line }}</p> }
        }
        @case ('runbooks') {
          <p class="muted small">What each kind of finding does: ignore (not posted), digest (a reply),
            notify (also sent to you), issue (a bug to file; also sent to you).</p>
          <form (ngSubmit)="saveRules(o)">
            <table>
              <thead><tr><th scope="col">Rule</th><th scope="col">Action</th><th scope="col">What to do</th></tr></thead>
              <tbody>
                @for (r of o.rules; track r.id) {
                  <tr>
                    <th scope="row"><code>{{ r.id }}</code></th>
                    <td><select [name]="'rule-' + r.id" [ngModel]="chosen()[r.id] ?? r.action" (ngModelChange)="choose(r.id, $event)">
                      @for (a of actions; track a) { <option [value]="a">{{ a }}{{ a === r.default ? ' (default)' : '' }}</option> }
                    </select></td>
                    <td class="small">{{ r.todo }}</td>
                  </tr>
                }
              </tbody>
            </table>
            <button class="btn" type="submit" [disabled]="busy()">Save rules</button>
            <button class="btn btn--quiet" type="button" (click)="resetRules(o)" [disabled]="busy()">Reset all to defaults</button>
          </form>
        }
        @case ('notify') {
          <p>Set up: {{ o.notify.channels.length ? o.notify.channels.join(', ') : 'none yet (see Setup)' }}.</p>
          <p><button class="btn" type="button" (click)="check()" [disabled]="busy() || !bot()?.ready">Send a test notice</button></p>
          @if (bot()?.last_check; as c) { <p class="small">{{ c.ok ? '✓' : '✗' }} {{ c.summary }}</p> }
          <h2>Last deliveries</h2>
          @for (line of o.notify.log.slice().reverse(); track $index) { <p class="small"><code>{{ line }}</code></p> }
          @empty { <p class="muted">Nothing sent yet.</p> }
        }
        @case ('history') { <app-sre-history /><app-sre-actions /> }
        @case ('setup') {
          @if (bot(); as b) {
            <p class="small">The Mastodon server, key and on/off switch are on the <a routerLink="/bots">Bots</a> page.
              Each setting below says where its value comes from.</p>
            <form (ngSubmit)="saveSettings(b)">
              <app-setting-fields [fields]="b.settings" prefix="sre" [(values)]="values" />
              <button class="btn" type="submit" [disabled]="busy()">Save SRE settings</button>
            </form>
          }
        }
      }
    } @else if (!error()) { <p class="muted">Loading…</p> }
  `,
})
export class SrePage implements OnInit {
  private readonly api = inject(Api);
  private readonly session = inject(Session);
  protected readonly tabs: [Tab, string][] = [
    ['fleet', 'Fleet'], ['findings', 'Findings'], ['runbooks', 'Runbooks'],
    ['notify', 'Notifications'], ['history', 'History'], ['setup', 'Setup'],
  ];
  protected readonly actions = ['ignore', 'digest', 'notify', 'issue'];
  protected readonly tab = signal<Tab>('fleet');
  protected readonly o = signal<Overview | null>(null);
  protected readonly bot = signal<Bot | null>(null);
  protected readonly chosen = signal<Record<string, string>>({});
  protected readonly values = signal<Record<string, string>>({});
  protected readonly busy = signal(false);
  protected readonly error = signal('');

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  private async load(): Promise<void> {
    try {
      const [overview, bots] = await Promise.all([
        this.api.get<Overview>('/api/v1/bots/sre/overview'),
        this.session.bots(),
      ]);
      this.o.set(overview);
      const sre = bots.find((b) => b.id === 'sre') ?? null;
      this.bot.set(sre);
      if (sre) this.values.set(initialValues(sre.settings));
      this.chosen.set({});
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected reading(now: number | null, usual: number | null, unit: string, readings: number): string {
    if (now === null) return '—';
    if (usual === null) return `${now}${unit} (learning, ${readings} of 3)`;
    return `${now}${unit}, usually ${usual}${unit}`;
  }

  protected boardUrl(o: Overview, host: string): string {
    return o.boards.find((b) => b.host === host)?.url ?? `https://${host}`;
  }

  protected choose(id: string, action: string): void {
    this.chosen.update((c) => ({ ...c, [id]: action }));
  }

  private async saveSetting(settings: Record<string, string>): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await this.session.update('sre', { settings });
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }

  protected saveRules(o: Overview): Promise<void> {
    return this.saveSetting({ runbook_rules: overrides(o.rules, this.chosen()) });
  }

  protected resetRules(o: Overview): Promise<void> {
    return this.saveSetting({ runbook_rules: overrides(o.rules.map((r) => ({ ...r, action: r.default })), {}) });
  }

  protected saveSettings(b: Bot): Promise<void> {
    return this.saveSetting(changedValues(b.settings, this.values()));
  }

  protected async check(): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await this.session.request('sre', 'check');
      // The check runs on the bot's worker; its result lands in a few seconds.
      for (let i = 0; i < 10; i++) {
        await new Promise((r) => setTimeout(r, 1500));
        const sre = (await this.session.bots()).find((b) => b.id === 'sre') ?? null;
        this.bot.set(sre);
        if (sre && !sre.running && !sre.queued) break;
      }
      this.o.set(await this.api.get<Overview>('/api/v1/bots/sre/overview'));
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
