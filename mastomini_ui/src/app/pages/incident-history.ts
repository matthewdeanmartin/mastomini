import { Component, input } from '@angular/core';
import { DecimalPipe } from '@angular/common';

export interface Incident {
  first_ms: number; last_ms: number; count: number; duration_ms: number; bytes: number; code: number; kind: string;
}

/** miniframework's incident history (also served at /.well-known/incidents). */
export interface IncidentHistory {
  boot_id: number; reset_reason: string; retained_bytes: number; volatile: boolean;
  dropped: number; overwritten: number;
  high_water: number[]; last_handshake_ms: number; max_handshake_ms: number;
  counters: { kind: string; count: number }[];
  events: Incident[];
  previous: { boot_id: number; events: Incident[]; counters: { kind: string; count: number }[] } | null;
  samples: { at_ms: number; free_heap: number; largest_block: number; rssi: number | null;
    tls_gap_ms: number; http_gap_ms: number; pending_tls: number; tls_clients: number; http_clients: number }[];
}

@Component({
  selector: 'app-incident-history',
  imports: [DecimalPipe],
  template: `
    @let h = history();
    <h2>Recent board incidents</h2>
    <p>Kept on the board even when this page is closed.
      @if (h.volatile) { RAM only: history clears on restart. }
      @else { The newest entries survive a crash or restart (not power loss). }
      Also at <a href="/.well-known/incidents">/.well-known/incidents</a>.</p>
    <p>Boot {{ h.boot_id }} · {{ h.retained_bytes }} bytes reserved · {{ h.overwritten }} events overwritten ·
      {{ h.dropped }} event/sample updates missed while busy.</p>
    <p>Last / longest TLS handshake: {{ h.last_handshake_ms }} / {{ h.max_handshake_ms }} ms.
      Peak connections (pending TLS / HTTPS / HTTP): {{ h.high_water.join(' / ') }}.</p>
    <button class="btn btn--quiet" type="button" (click)="download()">Download incident history</button>
    @if (h.events.length) {
      <div class="table-scroll"><table>
        <thead><tr><th>Latest uptime</th><th>Event</th><th>Code</th><th>Count</th><th>Longest duration</th><th>Bytes</th></tr></thead>
        <tbody>@for (e of h.events; track $index) {
          <tr><td>{{ e.last_ms / 1000 | number:'1.1-1' }} s</td><td>{{ label(e.kind) }}</td>
            <td>{{ code(e.kind, e.code) }}</td><td>{{ e.count }}</td><td>{{ e.duration_ms }} ms</td>
            <td>{{ e.bytes || '' }}</td></tr>
        }</tbody>
      </table></div>
    } @else { <p>No incidents retained for this boot.</p> }
    @if (h.previous; as p) {
      <details open><summary>Before the last restart ({{ h.reset_reason || 'unknown reason' }})</summary>
        <div class="table-scroll"><table>
          <thead><tr><th>Latest uptime</th><th>Event</th><th>Code</th><th>Count</th><th>Bytes</th></tr></thead>
          <tbody>@for (e of p.events; track $index) {
            <tr><td>{{ e.last_ms / 1000 | number:'1.1-1' }} s</td><td>{{ label(e.kind) }}</td>
              <td>{{ code(e.kind, e.code) }}</td><td>{{ e.count }}</td><td>{{ e.bytes || '' }}</td></tr>
          }</tbody>
        </table></div>
        @for (c of p.counters; track c.kind) { <p>{{ label(c.kind) }}: {{ c.count }}</p> }
      </details>
    }
    <details><summary>Recent health samples (about 160 seconds)</summary>
      <div class="table-scroll"><table>
        <thead><tr><th>Uptime</th><th>Free / largest KiB</th><th>Wi-Fi dBm</th><th>TLS / HTTP gap ms</th><th>Pending / HTTPS / HTTP</th></tr></thead>
        <tbody>@for (s of h.samples; track $index) {
          <tr><td>{{ s.at_ms / 1000 | number:'1.0-0' }} s</td>
            <td>{{ s.free_heap / 1024 | number:'1.0-0' }} / {{ s.largest_block / 1024 | number:'1.0-0' }}</td>
            <td>{{ s.rssi ?? 'disconnected / unavailable' }}</td><td>{{ s.tls_gap_ms }} / {{ s.http_gap_ms }}</td>
            <td>{{ s.pending_tls }} / {{ s.tls_clients }} / {{ s.http_clients }}</td></tr>
        }</tbody>
      </table></div>
      <p>A worker gap means progress stopped; it does not by itself identify the cause.</p>
    </details>
    <details><summary>Event counts since restart</summary>
      @for (c of h.counters; track c.kind) { @if (c.count) { <p>{{ label(c.kind) }}: {{ c.count }}</p> } }
    </details>
  `,
  styles: `.table-scroll { overflow-x:auto } table { width:100%; border-collapse:collapse }
    td, th { text-align:left; padding:.4rem; white-space:nowrap; border-bottom:1px solid #8885 }
    details { margin-block:1rem }`,
})
export class IncidentHistoryPanel {
  readonly history = input.required<IncidentHistory>();
  protected label(kind: string): string { return kind.replaceAll('_', ' '); }
  protected code(kind: string, code: number): string {
    if (kind === 'worker_stalled' || kind === 'worker_recovered') return code === 0 ? 'TLS worker' : 'HTTP worker';
    if (kind === 'request_timeout') return code === 0 ? 'receiving request' : 'sending response';
    if (kind === 'heap_low') return `${code} open connections`;
    if (kind === 'allocation_failed') return code === 0 ? 'response buffer' : `caps 0x${code.toString(16)}`;
    if (kind === 'tls_connection_failed' || kind === 'tls_failed') return code < 0 ? `-0x${(-code).toString(16)}` : String(code);
    return String(code);
  }
  protected download(): void {
    const url = URL.createObjectURL(new Blob([JSON.stringify(this.history(), null, 2)], { type: 'application/json' }));
    const a = document.createElement('a');
    a.href = url; a.download = `board-incidents-${this.history().boot_id}.json`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
}
