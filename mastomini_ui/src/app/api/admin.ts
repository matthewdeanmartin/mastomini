import { Injectable, inject } from '@angular/core';
import { Api, Body, Page } from './api';
import { Account } from './models';

export interface AdminAccount {
  id: string;
  username: string;
  account: Account;
  disabled: boolean;
  silenced: boolean;
  suspended: boolean;
  sensitized: boolean;
}
export interface Rule {
  id: string;
  text: string;
}
export type Category = 'other' | 'spam' | 'violation';
export interface Report {
  id: string;
  created_at: string;
  updated_at: string;
  action_taken: boolean;
  action_taken_at: string | null;
  category: Category;
  comment: string;
  account: AdminAccount | null;
  target_account: AdminAccount | null;
  assigned_account: AdminAccount | null;
  action_taken_by_account: AdminAccount | null;
  rules: Rule[];
  statuses: { id: string; content: string; spoiler_text: string; account: Account }[];
}
export type ReportAction = 'assign_to_self' | 'unassign' | 'resolve' | 'reopen';
export type AccountAction =
  | 'disable'
  | 'enable'
  | 'silence'
  | 'unsilence'
  | 'suspend'
  | 'unsuspend'
  | 'sensitive'
  | 'unsensitive';
export interface Announcement {
  id: string;
  text: string;
  content: string;
  published: boolean;
  all_day: boolean;
  starts_at: string | null;
  ends_at: string | null;
  updated_at: string;
}

const REPORTS = '/api/v1/admin/reports';
const ANNOUNCEMENTS = '/api/v1/admin/announcements';
const idPath = (base: string, id: string) => `${base}/${encodeURIComponent(id)}`;

@Injectable({ providedIn: 'root' })
export class Admin {
  private readonly api = inject(Api);

  reports(path: string, signal?: AbortSignal): Promise<Page<Report>> {
    return this.api.page(path, signal);
  }
  report(id: string): Promise<Report> {
    return this.api.get(idPath(REPORTS, id));
  }
  reportAction(id: string, action: ReportAction): Promise<Report> {
    return this.api.post(`${idPath(REPORTS, id)}/${action}`);
  }
  classify(id: string, category: Category, ruleIds: string[]): Promise<Report> {
    // Params preserves a named empty value; an empty JSON array has no entries.
    return this.api.put(idPath(REPORTS, id), {
      category,
      rule_ids: ruleIds.length ? ruleIds : [''],
    });
  }
  accountAction(id: string, action: AccountAction): Promise<unknown> {
    const base = idPath('/api/v1/admin/accounts', id);
    return ['enable', 'unsilence', 'unsuspend', 'unsensitive'].includes(action)
      ? this.api.post(`${base}/${action}`)
      : this.api.post(`${base}/action`, { type: action });
  }
  deleteStatus(id: string): Promise<unknown> {
    return this.api.delete(idPath('/api/mastomini/v1/admin/statuses', id));
  }
  rules(): Promise<Rule[]> {
    return this.api.get('/api/v1/instance/rules');
  }
  announcements(): Promise<Announcement[]> {
    return this.api.get(ANNOUNCEMENTS);
  }
  saveAnnouncement(id: string | null, body: Body): Promise<Announcement> {
    return id ? this.api.put(idPath(ANNOUNCEMENTS, id), body) : this.api.post(ANNOUNCEMENTS, body);
  }
  publish(id: string, published: boolean): Promise<Announcement> {
    return this.api.post(`${idPath(ANNOUNCEMENTS, id)}/${published ? 'publish' : 'unpublish'}`);
  }
  deleteAnnouncement(id: string): Promise<unknown> {
    return this.api.delete(idPath(ANNOUNCEMENTS, id));
  }
}
