import { Component, inject, signal } from '@angular/core';
import { DatePipe } from '@angular/common';
import { Api, describe } from '../api/api';

interface History {
  hosts: string[];
  days: { day: number; at_ms: number; statuses: string }[];
}

@Component({
  selector: 'app-sre-history',
  imports: [DatePipe],
  template: `
    <p><button class="btn btn--quiet" type="button" (click)="load()" [disabled]="busy()">
      {{ history() ? 'Refresh health history' : 'Show seven-day health history' }}
    </button></p>
    @if (error()) { <p class="error">{{ error() }}</p> }
    @if (adminDigest()) { <p class="small">Last completed weekly digest: {{ adminDigest() }}</p> }
    @if (history(); as h) {
      <p class="muted small">Latest observation per UTC day, including runs whose posts failed.
        Missing observations are unknown. Connection checks do not record history.</p>
      @if (h.days.length) {
        <div style="overflow-x:auto">
          <table>
            <thead><tr><th scope="col">Board</th>
              @for (day of h.days; track day.day) {
                <th scope="col">{{ day.at_ms | date:'MMM d, HH:mm':'UTC' }} UTC</th>
              }
            </tr></thead>
            <tbody>
              @for (host of h.hosts; track $index; let i = $index) {
                <tr><th scope="row">{{ host }}</th>
                  @for (day of h.days; track day.day) { <td>{{ label(day.statuses[i]) }}</td> }
                </tr>
              }
            </tbody>
          </table>
        </div>
      } @else { <p>No observations in the last seven UTC days.</p> }
    }
  `,
})
export class SreHistory {
  private readonly api = inject(Api);
  protected readonly history = signal<History | null>(null);
  protected readonly busy = signal(false);
  protected readonly error = signal('');
  protected readonly adminDigest = signal<string | null>(null);

  protected label(code: string): string {
    return ({ o: 'OK', w: 'Warning', f: 'Fail', d: 'Unreachable', n: 'No health endpoint' } as Record<string, string>)[code] ?? 'Not observed';
  }

  protected async load(): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    this.error.set('');
    try {
      this.history.set(await this.api.get<History>('/api/v1/bots/sre/history'));
      const admin = await this.api.get<{summary: string | null}>('/api/v1/bots/sre/admin-digest');
      this.adminDigest.set(admin.summary);
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
