import { Component, signal, inject } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { describe } from '../api/api';
import { Household } from '../api/household';

/** The passphrase rule the board enforces (domain/transfer.rs). */
export const PASSPHRASE_MIN = 12;

/**
 * Move the household to another board (spec/board_migration_plan.md): the
 * owner downloads an encrypted export, then restores it on the new board's
 * setup page. Nothing on this board changes.
 */
@Component({
  selector: 'app-move',
  imports: [FormsModule],
  template: `
    <h1>Move to another board</h1>
    <p>
      Download the whole household (every member, password, post and signed-in device) as one
      encrypted file, then restore it on a new board. This board is not changed.
    </p>

    <form class="panel" (ngSubmit)="exportHousehold()">
      <label for="password">Your password</label>
      <input id="password" name="password" type="password" autocomplete="current-password"
             [(ngModel)]="password" required />
      <label for="passphrase">Passphrase for the file</label>
      <input id="passphrase" name="passphrase" type="password" autocomplete="new-password"
             [(ngModel)]="passphrase" required [minlength]="min" />
      <label for="again">Passphrase again</label>
      <input id="again" name="again" type="password" autocomplete="new-password"
             [(ngModel)]="again" required />
      <p class="muted small">
        At least {{ min }} characters. The file holds password hashes and every signed-in device:
        anyone with it and the passphrase can restore your household. Keep both safe; the
        passphrase cannot be recovered.
      </p>
      <button class="btn" type="submit" [disabled]="busy()">
        {{ busy() ? 'Exporting… (up to a minute)' : 'Download export' }}
      </button>
    </form>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }

    @if (done(); as file) {
      <section class="panel" role="status">
        <p><strong>Saved {{ file }}.</strong></p>
        <h2>On the new board</h2>
        <ol>
          <li>Start the new board; before it is set up it shows the setup page.</li>
          <li>Choose <em>Restore a household from an export</em>, pick the file and enter the
            passphrase.</li>
        </ol>
        <h3>Same name as this board</h3>
        <p>
          Phones and apps keep working without signing in again, if the new board is built with
          this board's name and household certificate. Switch this board off (or reflash it under
          another name) before the new one starts, so the two never answer to the same name.
        </p>
        <h3>A new name</h3>
        <p>
          Everything moves, but each phone and app signs in again at the new address, and devices
          trust the new board's certificate as before.
        </p>
        <p class="muted small">
          Posts and changes made here after this export are not in the file: export again just
          before you switch.
        </p>
      </section>
    }
  `,
})
export class MovePage {
  private readonly household = inject(Household);
  protected readonly min = PASSPHRASE_MIN;
  protected password = '';
  protected passphrase = '';
  protected again = '';
  protected readonly busy = signal(false);
  protected readonly error = signal('');
  protected readonly done = signal('');

  protected async exportHousehold(): Promise<void> {
    this.error.set('');
    this.done.set('');
    if ([...this.passphrase].length < PASSPHRASE_MIN) {
      this.error.set(`The passphrase must be at least ${PASSPHRASE_MIN} characters.`);
      return;
    }
    if (this.passphrase !== this.again) {
      this.error.set('The two passphrases are different.');
      return;
    }
    this.busy.set(true);
    try {
      const { blob, filename } = await this.household.exportHousehold(this.password, this.passphrase);
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      this.done.set(filename);
      this.password = '';
      this.passphrase = '';
      this.again = '';
    } catch (e) {
      this.error.set(describe(e));
    } finally {
      this.busy.set(false);
    }
  }
}
