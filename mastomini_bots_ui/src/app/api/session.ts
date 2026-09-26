// The admin's session and every call the app makes.

import { Injectable, inject, signal } from '@angular/core';
import { Router } from '@angular/router';

import { Api } from './api';
import { ActivityEvent, Bot, BotUpdate, Diag, OpenRouterStatus, Status } from './models';

const V1 = '/api/v1';

@Injectable({ providedIn: 'root' })
export class Session {
  private readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly status = signal<Status | null>(null);
  readonly signedIn = signal(false);

  constructor() {
    this.api.onUnauthorized = () => {
      this.forget();
      void this.router.navigateByUrl('/signin');
    };
  }

  /** Ask the board who we are; true when signed in. */
  async start(): Promise<boolean> {
    const status = await this.api.get<Status>(`${V1}/status`);
    this.status.set(status);
    this.signedIn.set(status.signed_in);
    if (!status.signed_in) this.api.setToken(null);
    return status.signed_in;
  }

  async setup(password: string): Promise<void> {
    const { token } = await this.api.post<{ token: string }>(`${V1}/setup`, { password });
    this.remember(token);
  }

  async login(password: string): Promise<void> {
    const { token } = await this.api.post<{ token: string }>(`${V1}/login`, { password });
    this.remember(token);
  }

  async logout(): Promise<void> {
    try {
      await this.api.post(`${V1}/logout`);
    } finally {
      this.forget();
    }
  }

  changePassword(current: string, next: string): Promise<void> {
    return this.api.post(`${V1}/password`, { current, new: next });
  }

  bots(): Promise<Bot[]> {
    return this.api.get(`${V1}/bots`);
  }

  update(id: string, update: BotUpdate): Promise<Bot> {
    return this.api.put(`${V1}/bots/${encodeURIComponent(id)}`, { ...update });
  }

  /** `run` posts now; `check` only signs in with the API key. */
  request(id: string, action: 'run' | 'check'): Promise<Bot> {
    return this.api.post(`${V1}/bots/${encodeURIComponent(id)}/${action}`);
  }

  activity(since = 0, limit = 100): Promise<ActivityEvent[]> {
    return this.api.get(`${V1}/activity?since=${since}&limit=${limit}`);
  }

  openRouter(): Promise<OpenRouterStatus> {
    return this.api.get(`${V1}/integrations/openrouter`);
  }

  /** An empty key removes it; omitted fields stay. */
  setOpenRouter(update: { key?: string; model?: string }): Promise<OpenRouterStatus> {
    return this.api.put(`${V1}/integrations/openrouter`, { ...update });
  }

  diag(): Promise<Diag> {
    return this.api.get(`${V1}/diag`);
  }

  private remember(token: string): void {
    this.api.setToken(token);
    this.signedIn.set(true);
  }

  private forget(): void {
    this.api.setToken(null);
    this.signedIn.set(false);
  }
}
