// `/api/mastomini/v1`: what the Mastodon API has no room for (spec/06).

import { Injectable, inject } from '@angular/core';

import { Api } from './api';
import { Code, Device, Diag, Member, Security, ServerSettings, Status } from './models';

const V1 = '/api/mastomini/v1';

/** Actions on a member that take no arguments. */
export type MemberAction =
  | 'disable'
  | 'enable'
  | 'silence'
  | 'unsilence'
  | 'suspend'
  | 'unsuspend'
  | 'reset';

@Injectable({ providedIn: 'root' })
export class Household {
  private readonly api = inject(Api);

  status(): Promise<Status> {
    return this.api.get(`${V1}/status`);
  }

  diag(): Promise<Diag> {
    return this.api.get(`${V1}/diag`);
  }

  /** Set the board's clock from this device (only while it isn't synced). */
  setClock(ms: number): Promise<void> {
    return this.api.post(`${V1}/clock`, { ms });
  }

  security(): Promise<Security> {
    return this.api.get(`${V1}/admin/security`);
  }

  // Me

  devices(): Promise<Device[]> {
    return this.api.get(`${V1}/me/devices`);
  }

  revokeDevice(id: string): Promise<void> {
    return this.api.delete(`${V1}/me/devices/${encodeURIComponent(id)}`);
  }

  changePassword(current: string, next: string): Promise<void> {
    return this.api.post(`${V1}/me/password`, { current, new: next });
  }

  signOutEverywhere(): Promise<void> {
    return this.api.post(`${V1}/me/sign_out_everywhere`);
  }

  // Admin

  members(): Promise<Member[]> {
    return this.api.get(`${V1}/admin/members`);
  }

  /** `reset` returns the new code; the others return the updated member. */
  act(id: string, action: MemberAction): Promise<Member | Code> {
    return this.api.post(`${V1}/admin/members/${encodeURIComponent(id)}/${action}`);
  }

  setRole(id: string, role: 'admin' | 'member'): Promise<Member> {
    return this.api.post(`${V1}/admin/members/${encodeURIComponent(id)}/role`, { role });
  }

  deleteMember(id: string, confirm: string): Promise<void> {
    return this.api.delete(`${V1}/admin/members/${encodeURIComponent(id)}`, { confirm });
  }

  /** Create a member with a password the admin chooses (invites are better). */
  addMember(username: string, password: string, display_name: string): Promise<Member> {
    return this.api.post(`${V1}/admin/members`, { username, password, display_name });
  }

  invite(): Promise<Code> {
    return this.api.post(`${V1}/admin/invites`);
  }

  codes(): Promise<Code[]> {
    return this.api.get(`${V1}/admin/codes`);
  }

  revokeCode(id: string): Promise<void> {
    return this.api.delete(`${V1}/admin/codes/${encodeURIComponent(id)}`);
  }

  server(): Promise<ServerSettings> {
    return this.api.get(`${V1}/admin/server`);
  }

  updateServer(update: Partial<Pick<ServerSettings, 'title' | 'description' | 'rules' | 'terms'>>): Promise<ServerSettings> {
    return this.api.put(`${V1}/admin/server`, update);
  }
}
