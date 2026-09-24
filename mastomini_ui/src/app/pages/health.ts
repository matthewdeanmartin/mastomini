import { Component, OnInit, inject, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';

import { describe } from '../api/api';
import { Household } from '../api/household';
import { Diag } from '../api/models';
import { percent, when } from './format';

/** Board diagnostics (`/diag`) and the clock fallback (spec/05 "Time"). */
@Component({
  selector: 'app-health',
  imports: [DecimalPipe],
  template: `
    <h1>Health</h1>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (d(); as d) {
      @if (!d.store.available) {
        <section class="panel">
          <strong class="error">Storage failed; the board is read-only until it restarts.</strong>
          <p class="muted small">{{ d.store.problem }}</p>
        </section>
      }

      <h2>Clock</h2>
      <section class="panel">
        @switch (d.clock.source) {
          @case ('synced') {
            <p>Set: {{ when(d.clock.now!) }}.</p>
          }
          @case ('manual') {
            <p>Set by hand to about {{ when(d.clock.now!) }}. It switches to internet time as soon
              as the board can reach it.</p>
          }
          @default {
            <p><strong>Not set.</strong> The board gets the time from the internet; until it has
              it, nobody can post. If the internet is down, set it from this device.</p>
          }
        }
        @if (d.clock.source !== 'synced') {
          <p class="muted small">This device says it is {{ now() }}.</p>
          <button class="btn" type="button" (click)="setClock()">Set the board's clock</button>
        }
      </section>

      <h2>Storage</h2>
      <section class="panel">
        <dl class="facts">
          <dt>Used</dt>
          <dd>
            {{ percent(d.store.used) }}
            @if (d.store.entries_used !== null) {
              <span class="muted">({{ d.store.entries_used | number }} of
                {{ d.store.entries_total | number }} entries)</span>
            }
          </dd>
          <dt>Oldest post kept</dt>
          <dd>{{ d.store.oldest_post_at ? when(d.store.oldest_post_at) : 'none yet' }}</dd>
          <dt>Old posts removed</dt>
          <dd>{{ d.store.evictions_since_boot }} since the board started</dd>
          <dt>Repairs at start</dt>
          <dd>{{ d.store.repairs_at_boot }}</dd>
        </dl>
        <p class="muted small">
          Above {{ percent(d.store.low_watermark) }}, the oldest posts (except pinned ones) are
          removed to make room.
        </p>
      </section>

      <h2>Board</h2>
      <section class="panel">
        <dl class="facts">
          <dt>Running for</dt>
          <dd>{{ duration(d.platform.uptime_ms) }}</dd>
          @if (d.platform.reset_reason) {
            <dt>Last restart</dt>
            <dd>{{ d.platform.reset_reason }}</dd>
          }
          @if (d.platform.heap_internal_free !== null) {
            <dt>Internal memory free</dt>
            <dd>{{ kib(d.platform.heap_internal_free) }} (lowest {{ kib(d.platform.heap_internal_min_free) }},
              largest block {{ kib(d.platform.heap_internal_largest_block) }})</dd>
          }
          @if (d.platform.psram_free !== null) {
            <dt>PSRAM free</dt>
            <dd>{{ kib(d.platform.psram_free) }} (lowest {{ kib(d.platform.psram_min_free) }})</dd>
          }
          @if (d.platform.wifi_rssi !== null) {
            <dt>Wi-Fi signal</dt>
            <dd>{{ d.platform.wifi_rssi }} dBm ({{ strength(d.platform.wifi_rssi) }})</dd>
          }
          <dt>Runs on</dt>
          <dd>{{ d.platform.target }}, version {{ d.version }}</dd>
        </dl>
      </section>

      <h2>Capacity</h2>
      <section class="panel">
        <dl class="facts">
          @for (row of records(d); track row.name) {
            <dt>{{ row.name }}</dt>
            <dd>{{ row.used | number }} of {{ row.limit | number }}</dd>
          }
          <dt>Writes this hour</dt>
          <dd>{{ d.governor.writes_this_hour }} of {{ d.governor.limit_per_hour }}
            <span class="muted">({{ d.governor.limit_per_member_hour }} per member)</span></dd>
        </dl>
      </section>
      <button class="btn btn--quiet" type="button" (click)="load()">Refresh</button>
    } @else if (!error()) {
      <p class="muted">Loading…</p>
    }
  `,
})
export class HealthPage implements OnInit {
  private readonly household = inject(Household);
  protected readonly d = signal<Diag | null>(null);
  protected readonly error = signal('');
  protected readonly percent = percent;
  protected readonly when = when;

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  protected async load(): Promise<void> {
    try {
      this.d.set(await this.household.diag());
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected now(): string {
    return new Date().toLocaleString();
  }

  protected async setClock(): Promise<void> {
    this.error.set('');
    if (!confirm(`Set the board's clock to ${this.now()}?`)) return;
    try {
      await this.household.setClock(Date.now());
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected records(d: Diag): { name: string; used: number; limit: number }[] {
    const names: Record<string, string> = {
      accounts: 'Members',
      statuses: 'Posts',
      boosts: 'Boosts',
      reactions: 'Favourites, bookmarks and pins',
      apps: 'Registered apps',
      tokens: 'Signed-in devices',
      reports: 'Reports',
      codes: 'Open invite and reset links',
      notifications: 'Notifications (kept until restart)',
    };
    // In this order; the server's JSON object order is alphabetical.
    return Object.keys(names)
      .filter((key) => d.records[key])
      .map((key) => ({ name: names[key], used: d.records[key][0], limit: d.records[key][1] }));
  }

  protected kib(bytes: number | null): string {
    return bytes === null ? '?' : `${Math.round(bytes / 1024)} KiB`;
  }

  protected duration(ms: number): string {
    const minutes = Math.floor(ms / 60_000);
    const [d, h, m] = [Math.floor(minutes / 1440), Math.floor((minutes % 1440) / 60), minutes % 60];
    return d ? `${d} d ${h} h` : h ? `${h} h ${m} min` : `${m} min`;
  }

  protected strength(rssi: number): string {
    return rssi >= -60 ? 'good' : rssi >= -72 ? 'fair' : 'weak';
  }
}
