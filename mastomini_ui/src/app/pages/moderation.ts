import { Component, OnDestroy, OnInit, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { RouterLink } from '@angular/router';
import { AccountAction, Admin, Category, Report, ReportAction, Rule } from '../api/admin';
import { ApiError, describe } from '../api/api';
import { Auth } from '../api/auth';
import { Household } from '../api/household';
import { Member } from '../api/models';
import { when } from './format';

@Component({
  selector: 'app-moderation',
  imports: [FormsModule, RouterLink],
  template: `
    <h1>Moderation</h1>
    <p class="lede">Review household reports, take action, and resolve each case when finished.</p>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }
    @if (notice()) {
      <p class="ok" role="status">{{ notice() }}</p>
    }
    <form class="panel" (ngSubmit)="filter()">
      <fieldset [disabled]="busy() || loading()">
        <label for="report-state">Report state</label>
        <select id="report-state" name="state" [(ngModel)]="resolved">
          <option value="false">Open</option>
          <option value="true">Resolved</option>
        </select>
        <label for="reporter">Reported by</label>
        <select id="reporter" name="reporter" [(ngModel)]="reporter">
          <option value="">Anyone</option>
          @for (m of members(); track m.id) {
            <option [value]="m.id">{{ m.username }}</option>
          }
        </select>
        <label for="report-target">Reported member</label>
        <select id="report-target" name="target" [(ngModel)]="target">
          <option value="">Anyone</option>
          @for (m of members(); track m.id) {
            <option [value]="m.id">{{ m.username }}</option>
          }
        </select>
        <button class="btn" type="submit">Apply filters / refresh</button>
      </fieldset>
    </form>
    @if (loading()) {
      <p role="status">Loading reports…</p>
    }
    @if (loaded()) {
      <h2>{{ showingResolved() ? 'Resolved reports' : 'Open reports' }}</h2>
      <p class="muted small">
        {{ rows().length }} reports on this page. Filters apply to the full queue.
      </p>
      @if (!rows().length) {
        <p>No reports on this page. Try Previous or refresh the queue.</p>
      }
      <ul class="rows">
        @for (r of rows(); track r.id) {
          <li>
            <button class="link" [disabled]="busy() || loading()" (click)="open(r.id)">
              Report #{{ r.id }} · {{ r.target_account?.username || 'Unavailable member' }}
            </button>
            <p class="small muted">
              {{ when(r.created_at) }} · {{ r.category }} ·
              {{
                r.assigned_account ? 'Assigned to ' + r.assigned_account!.username : 'Unassigned'
              }}
            </p>
          </li>
        }
      </ul>
      <nav aria-label="Report pages">
        <button
          class="btn btn--quiet"
          [disabled]="busy() || loading() || !previous()"
          (click)="load(previous()!)"
        >
          Previous
        </button>
        <button
          class="btn btn--quiet"
          [disabled]="busy() || loading() || !next()"
          (click)="load(next()!)"
        >
          Next
        </button>
      </nav>
    }
    @if (selected(); as r) {
      <section class="panel" aria-label="Report detail" [attr.aria-busy]="busy()">
        <h2>Report #{{ r.id }}</h2>
        <p>{{ r.action_taken ? 'Resolved' : 'Open' }} · {{ when(r.created_at) }}</p>
        <p>
          By {{ r.account?.username || 'Unavailable member' }} about
          {{ r.target_account?.username || 'Unavailable member' }}
        </p>
        <p class="plain-preview">{{ r.comment || 'No comment provided.' }}</p>
        <p>Assigned: {{ r.assigned_account?.username || 'Nobody' }}</p>
        @if (r.action_taken_by_account) {
          <p>Resolved by {{ r.action_taken_by_account!.username }}</p>
        }
        <button class="btn btn--quiet" [disabled]="busy() || loading()" (click)="open(r.id)">
          Refresh case
        </button>
        <button
          class="btn btn--quiet"
          [disabled]="busy() || loading()"
          (click)="
            reportAction(r.assigned_account?.id === auth.me()?.id ? 'unassign' : 'assign_to_self')
          "
        >
          {{ r.assigned_account?.id === auth.me()?.id ? 'Unassign me' : 'Assign to me' }}
        </button>
        <form (ngSubmit)="classify()">
          <fieldset [disabled]="busy() || loading()">
            <label for="category">Category</label>
            <select id="category" name="category" [(ngModel)]="category">
              <option value="other">Other</option>
              <option value="spam">Spam</option>
              <option value="violation">Rule violation</option>
            </select>
            <fieldset>
              <legend>Rules</legend>
              @for (rule of rules(); track rule.id) {
                <label
                  ><input
                    type="checkbox"
                    [checked]="ruleIds.includes(rule.id)"
                    (change)="toggleRule(rule.id, $event)"
                  />
                  {{ rule.text }}</label
                >
              }
              @if (!rules().length) {
                <p class="muted">No server rules configured.</p>
              }
            </fieldset>
            <button class="btn btn--quiet" type="submit">Save classification</button>
          </fieldset>
        </form>
        <h2>Reported posts</h2>
        <p class="small muted">
          Only retained content visible to your admin account is shown. Unrelated direct messages
          remain private.
        </p>
        @if (!r.statuses.length) {
          <p>No reported posts are available to display.</p>
        }
        @for (post of r.statuses; track post.id) {
          <article class="panel">
            <p>
              <strong>{{ post.account.username }}</strong> · #{{ post.id }}
            </p>
            @if (post.spoiler_text) {
              <p>{{ post.spoiler_text }}</p>
            }
            <div class="reported-content" [innerHTML]="post.content"></div>
            <button
              class="btn btn--danger"
              [disabled]="busy() || loading()"
              (click)="deletePost(post.id)"
            >
              Delete post
            </button>
          </article>
        }
        @if (r.target_account; as account) {
          <h2>Member actions: {{ account.username }}</h2>
          <p class="small muted">
            Account actions do not resolve this report. Resolve it separately after reviewing the
            result.
          </p>
          <button
            class="btn btn--quiet"
            [disabled]="busy() || loading()"
            (click)="accountAction(account.disabled ? 'enable' : 'disable')"
          >
            {{ account.disabled ? 'Enable' : 'Disable' }}
          </button>
          <button
            class="btn btn--quiet"
            [disabled]="busy() || loading()"
            (click)="accountAction(account.silenced ? 'unsilence' : 'silence')"
          >
            {{ account.silenced ? 'Unsilence' : 'Silence' }}
          </button>
          <button
            class="btn btn--danger"
            [disabled]="busy() || loading()"
            (click)="accountAction(account.suspended ? 'unsuspend' : 'suspend')"
          >
            {{ account.suspended ? 'Unsuspend' : 'Suspend' }}
          </button>
          <button
            class="btn btn--quiet"
            [disabled]="busy() || loading()"
            (click)="accountAction(account.sensitized ? 'unsensitive' : 'sensitive')"
          >
            {{ account.sensitized ? 'Remove forced content warning' : 'Force content warning' }}
          </button>
          <p><a routerLink="/admin/members">Member roles, invites, and account deletion</a></p>
        }
        <button
          class="btn"
          [disabled]="busy() || loading()"
          (click)="reportAction(r.action_taken ? 'reopen' : 'resolve')"
        >
          {{ r.action_taken ? 'Reopen report' : 'Resolve report' }}
        </button>
      </section>
    }
  `,
})
export class ModerationPage implements OnInit, OnDestroy {
  private readonly admin = inject(Admin);
  private readonly household = inject(Household);
  readonly auth = inject(Auth);
  readonly when = when;
  readonly rows = signal<Report[]>([]);
  readonly members = signal<Member[]>([]);
  readonly rules = signal<Rule[]>([]);
  readonly selected = signal<Report | null>(null);
  readonly next = signal<string | null>(null);
  readonly previous = signal<string | null>(null);
  readonly error = signal('');
  readonly notice = signal('');
  readonly busy = signal(false);
  readonly loading = signal(false);
  readonly loaded = signal(false);
  readonly showingResolved = signal(false);
  resolved = 'false';
  reporter = '';
  target = '';
  category: Category = 'other';
  ruleIds: string[] = [];
  private path = '/api/v1/admin/reports?limit=10&resolved=false';
  private controller?: AbortController;
  private destroyed = false;

  async ngOnInit(): Promise<void> {
    try {
      this.members.set(await this.household.members());
      this.rules.set(await this.admin.rules());
    } catch (e) {
      this.error.set(describe(e));
    }
    await this.load(this.path, false);
  }
  ngOnDestroy(): void {
    this.destroyed = true;
    this.controller?.abort();
  }
  canLeave(): boolean {
    return !this.busy() && this.discardClassification();
  }
  private discardClassification(): boolean {
    const r = this.selected();
    return (
      !r ||
      (this.category === r.category &&
        [...this.ruleIds].sort().join() ===
          r.rules
            .map((v) => v.id)
            .sort()
            .join()) ||
      confirm('Discard unsaved report classification changes?')
    );
  }
  async filter(): Promise<void> {
    if (this.busy() || this.loading() || !this.discardClassification()) return;
    const query = new URLSearchParams({ limit: '10', resolved: this.resolved });
    if (this.reporter) query.set('account_id', this.reporter);
    if (this.target) query.set('target_account_id', this.target);
    this.selected.set(null);
    await this.load(`/api/v1/admin/reports?${query}`);
  }
  async load(path: string, clearError = true): Promise<void> {
    if (this.destroyed) return;
    this.controller?.abort();
    const controller = new AbortController();
    this.controller = controller;
    this.loading.set(true);
    if (clearError) this.error.set('');
    try {
      const page = await this.admin.reports(path, controller.signal);
      if (controller.signal.aborted || this.destroyed) return;
      this.rows.set(page.items);
      this.next.set(page.next);
      // A full final page can advertise one extra, empty page. Keep a way back.
      const cursor = new URL(path, location.origin).searchParams;
      this.previous.set(
        page.prev ?? (cursor.has('max_id') || cursor.has('min_id') ? this.path : null),
      );
      this.path = path;
      this.showingResolved.set(
        new URL(path, location.origin).searchParams.get('resolved') === 'true',
      );
      this.loaded.set(true);
    } catch (e) {
      if (!controller.signal.aborted) this.error.set(describe(e));
    } finally {
      if (!controller.signal.aborted) this.loading.set(false);
    }
  }
  private apply(report: Report): void {
    this.selected.set(report);
    this.category = report.category;
    this.ruleIds = report.rules.map((r) => r.id);
  }
  async open(id: string): Promise<void> {
    if (this.busy() || this.loading() || !this.discardClassification()) return;
    this.busy.set(true);
    this.error.set('');
    this.notice.set('');
    // Never leave an old case actionable after a failed refresh/deletion.
    this.selected.set(null);
    try {
      const report = await this.admin.report(id);
      if (!this.destroyed) this.apply(report);
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
  toggleRule(id: string, event: Event): void {
    this.ruleIds = (event.target as HTMLInputElement).checked
      ? [...this.ruleIds, id]
      : this.ruleIds.filter((r) => r !== id);
  }
  async classify(): Promise<void> {
    const r = this.selected();
    if (r)
      await this.mutate(
        () => this.admin.classify(r.id, this.category, this.ruleIds),
        'Classification saved.',
      );
  }
  async reportAction(action: ReportAction): Promise<void> {
    const r = this.selected();
    if (!r || !this.discardClassification()) return;
    if (
      (action === 'resolve' || action === 'reopen') &&
      !confirm(`${action === 'resolve' ? 'Resolve' : 'Reopen'} report #${r.id}?`)
    )
      return;
    await this.mutate(() => this.admin.reportAction(r.id, action), 'Report updated.');
  }
  async accountAction(action: AccountAction): Promise<void> {
    const r = this.selected();
    if (!r?.target_account || !this.discardClassification()) return;
    const effects: Record<AccountAction, string> = {
      disable: 'Prevent this member from using their account',
      enable: 'Restore access to this member’s account',
      silence: 'Hide this member from public timelines',
      unsilence: 'Remove the public timeline restriction',
      suspend: 'Suspend access and hide all of this member’s posts',
      unsuspend: 'Restore this suspended member',
      sensitive: 'Mark this member’s posts as sensitive',
      unsensitive: 'Remove forced sensitive marking',
    };
    if (
      !confirm(
        `${effects[action]} (${r.target_account.username})? This does not resolve the report.`,
      )
    )
      return;
    await this.mutate(
      () => this.admin.accountAction(r.target_account!.id, action),
      'Member action saved. Review the case before resolving it.',
    );
  }
  async deletePost(id: string): Promise<void> {
    if (
      !this.discardClassification() ||
      !confirm(`Permanently delete post #${id}? This does not resolve the report.`)
    )
      return;
    await this.mutate(
      () => this.admin.deleteStatus(id),
      'Post deleted. The report remains in its current state.',
    );
  }
  private async mutate(action: () => Promise<unknown>, message: string): Promise<void> {
    const r = this.selected();
    if (!r || this.busy() || this.loading()) return;
    this.busy.set(true);
    this.error.set('');
    this.notice.set('');
    let committed = false;
    try {
      await action();
      committed = true;
      this.notice.set(message);
      this.apply(await this.admin.report(r.id));
      await this.load(this.path, false);
    } catch (e) {
      this.error.set(
        committed
          ? `${message} Refresh failed: ${describe(e)} Refresh the case before taking another action.`
          : `${describe(e)} Refresh the case to verify its state before retrying.`,
      );
      // Keep validation edits; ambiguous mutations and stale resources require a fresh read.
      if (
        committed ||
        !(e instanceof ApiError) ||
        [0, 401, 403, 404].includes(e.status) ||
        e.status >= 500
      )
        this.selected.set(null);
    } finally {
      this.busy.set(false);
    }
  }
}
