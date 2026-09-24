import { Component, OnInit, computed, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { toDataURL } from 'qrcode';

import { describe } from '../api/api';
import { Auth } from '../api/auth';
import { Household, MemberAction } from '../api/household';
import { Code, Member } from '../api/models';
import { copyText } from './clipboard';
import { when } from './format';

/** A link just created, shown with its QR code until dismissed. */
interface Fresh {
  code: Code;
  qr: string;
}

/**
 * Members, invite and reset links, and moderation. The rules mirror the
 * server's (spec/04 "Moderation"): nobody acts on themselves or the owner, and
 * only the owner acts on admins or changes roles.
 */
@Component({
  selector: 'app-members',
  imports: [FormsModule],
  template: `
    <h1>Members</h1>
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }

    <section class="panel">
      <h2 style="margin-top: 0">Invite someone</h2>
      <p class="muted small">
        Creates a link that works once, for 7 days. The new member opens it and chooses their own
        username and password.
      </p>
      <button class="btn" type="button" (click)="invite()">Create invite link</button>
      <details>
        <summary class="muted small">Or add someone with a password you choose</summary>
        <form (ngSubmit)="add()">
          <label for="new-username">Username</label>
          <input id="new-username" name="username" type="text" autocapitalize="none"
                 [(ngModel)]="newUsername" required />
          <label for="new-display">Display name (optional)</label>
          <input id="new-display" name="display" type="text" [(ngModel)]="newDisplayName" />
          <label for="new-password">Password</label>
          <input id="new-password" name="password" type="password" autocomplete="new-password"
                 [(ngModel)]="newPassword" required />
          <p class="muted small">
            You'll know their password until they change it, and with it their direct messages.
            An invite link avoids that.
          </p>
          <button class="btn btn--quiet" type="submit">Add member</button>
          @if (added()) {
            <span class="ok">Added {{ added() }}.</span>
          }
        </form>
      </details>
    </section>

    @if (fresh(); as f) {
      <section class="panel">
        <h2 style="margin-top: 0">
          @if (f.code.kind === 'invite') {
            Invite link
          } @else {
            Password reset link for {{ f.code.username }}
          }
        </h2>
        <p class="muted small">
          Send it by text or show the QR code. It works once and expires {{ when(f.code.expires_at) }}.
          @if (f.code.kind === 'reset') {
            Using it signs {{ f.code.username }} out everywhere, and their earlier direct messages can
            no longer be read.
          }
        </p>
        <img class="qr" [src]="f.qr" alt="QR code for the link" />
        <p class="link-box">{{ f.code.url }}</p>
        <button class="btn" type="button" (click)="copy(f.code.url!)">Copy link</button>
        <button class="btn btn--quiet" type="button" (click)="fresh.set(null)">Done</button>
        @if (copied()) {
          <span class="ok">Copied.</span>
        }
      </section>
    }

    <h2>Household</h2>
    <section class="panel">
      <ul class="rows">
        @for (m of members(); track m.id) {
          <li>
            <strong>{{ m.username }}</strong>
            @if (m.display_name) {
              <span class="muted">({{ m.display_name }})</span>
            }
            @if (m.role !== 'member') {
              <span class="tag">{{ m.role }}</span>
            }
            @if (m.disabled) {
              <span class="tag tag--warn">disabled</span>
            }
            @if (m.suspended) {
              <span class="tag tag--warn">suspended</span>
            }
            @if (m.silenced) {
              <span class="tag">silenced</span>
            }
            @if (canAct(m)) {
              <div>
                <button class="btn btn--quiet" type="button" (click)="act(m, 'reset')">
                  Reset password
                </button>
                <button class="btn btn--quiet" type="button"
                        (click)="act(m, m.disabled ? 'enable' : 'disable')">
                  {{ m.disabled ? 'Enable' : 'Disable' }}
                </button>
                <button class="btn btn--quiet" type="button"
                        (click)="act(m, m.silenced ? 'unsilence' : 'silence')">
                  {{ m.silenced ? 'Unsilence' : 'Silence' }}
                </button>
                <button class="btn btn--quiet" type="button"
                        (click)="act(m, m.suspended ? 'unsuspend' : 'suspend')">
                  {{ m.suspended ? 'Unsuspend' : 'Suspend' }}
                </button>
                @if (auth.isOwner()) {
                  <button class="btn btn--quiet" type="button" (click)="toggleRole(m)">
                    {{ m.role === 'admin' ? 'Make member' : 'Make admin' }}
                  </button>
                }
                <button class="btn btn--danger" type="button" (click)="remove(m)">Delete</button>
              </div>
            }
          </li>
        } @empty {
          <li class="muted">Loading…</li>
        }
      </ul>
    </section>
    <p class="muted small">
      Disable: can't sign in, apps stop working until enabled. Silence: hidden from people who
      don't follow them. Suspend: can't sign in and all their posts are hidden. Delete removes the
      account and everything it made, for good.
    </p>

    @if (codes().length) {
      <h2>Open links</h2>
      <section class="panel">
        <ul class="rows">
          @for (c of codes(); track c.id) {
            <li>
              @if (c.kind === 'invite') {
                Invite
              } @else {
                Password reset for <strong>{{ c.username }}</strong>
              }
              <div class="muted small">
                Created {{ when(c.created_at) }} · expires {{ when(c.expires_at) }}
              </div>
              <button class="btn btn--quiet" type="button" (click)="cancel(c)">Cancel link</button>
            </li>
          }
        </ul>
      </section>
    }
  `,
})
export class MembersPage implements OnInit {
  protected readonly auth = inject(Auth);
  private readonly household = inject(Household);
  protected readonly members = signal<Member[]>([]);
  protected readonly codes = signal<Code[]>([]);
  protected readonly fresh = signal<Fresh | null>(null);
  protected readonly copied = signal(false);
  protected readonly error = signal('');
  protected readonly when = when;
  protected readonly added = signal('');
  private readonly myId = computed(() => this.auth.me()?.id);
  protected newUsername = '';
  protected newDisplayName = '';
  protected newPassword = '';

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  private async load(): Promise<void> {
    try {
      const [members, codes] = await Promise.all([this.household.members(), this.household.codes()]);
      this.members.set(members);
      this.codes.set(codes);
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  protected canAct(m: Member): boolean {
    return (
      m.id !== this.myId() && m.role !== 'owner' && (m.role !== 'admin' || this.auth.isOwner())
    );
  }

  private async run(work: () => Promise<unknown>): Promise<void> {
    this.error.set('');
    try {
      await work();
      await this.load();
    } catch (e) {
      this.error.set(describe(e));
    }
  }

  private async show(code: Code): Promise<void> {
    this.copied.set(false);
    const qr = await toDataURL(code.url ?? '', { margin: 1, scale: 6 });
    this.fresh.set({ code, qr });
  }

  protected add(): Promise<void> {
    this.added.set('');
    const username = this.newUsername.trim().toLowerCase();
    return this.run(async () => {
      await this.household.addMember(username, this.newPassword, this.newDisplayName.trim());
      this.added.set(username);
      this.newUsername = this.newDisplayName = this.newPassword = '';
    });
  }

  protected invite(): Promise<void> {
    return this.run(async () => this.show(await this.household.invite()));
  }

  protected act(m: Member, action: MemberAction): Promise<void> {
    if (action === 'suspend' && !confirm(`Suspend ${m.username}? All their posts will be hidden.`)) {
      return Promise.resolve();
    }
    return this.run(async () => {
      const result = await this.household.act(m.id, action);
      if (action === 'reset') await this.show(result as Code);
    });
  }

  protected toggleRole(m: Member): Promise<void> {
    return this.run(() => this.household.setRole(m.id, m.role === 'admin' ? 'member' : 'admin'));
  }

  protected remove(m: Member): Promise<void> {
    const typed = prompt(
      `Delete ${m.username} and everything they posted, for good? Type their username to confirm.`,
    );
    if (typed === null) return Promise.resolve();
    return this.run(() => this.household.deleteMember(m.id, typed.trim()));
  }

  protected cancel(c: Code): Promise<void> {
    return this.run(() => this.household.revokeCode(c.id));
  }

  protected async copy(text: string): Promise<void> {
    await copyText(text);
    this.copied.set(true);
  }
}
