import { Component, input, model } from '@angular/core';
import { FormsModule } from '@angular/forms';

import { SettingField } from '../api/models';

/**
 * A bot's own settings, drawn from what its code declares. Every bot gets
 * the same form; no bot needs UI code. `values` holds the edited text by key.
 */
@Component({
  selector: 'app-setting-fields',
  imports: [FormsModule],
  template: `
    @for (f of fields(); track f.key) {
      @let id = prefix() + '-' + f.key;
      @switch (f.kind) {
        @case ('toggle') {
          <label>
            <input type="checkbox" [name]="id" [ngModel]="values()[f.key] === 'yes'"
                   (ngModelChange)="set(f.key, $event ? 'yes' : 'no')" />
            {{ f.label }}
          </label>
        }
        @default {
          <label [for]="id">{{ f.label }}{{ f.optional ? ' (optional)' : '' }}</label>
          @switch (f.kind) {
            @case ('long_text') {
              <textarea class="template" [id]="id" [name]="id" [ngModel]="values()[f.key]"
                        (ngModelChange)="set(f.key, $event)"></textarea>
              @if (f.placeholders?.length) {
                <div class="vars" title="Variables this text may use">
                  @for (p of f.placeholders; track p) {
                    <code>{{ '{{' + p + '}}' }}</code>
                  }
                </div>
              }
            }
            @case ('choice') {
              <select [id]="id" [name]="id" [ngModel]="values()[f.key]" (ngModelChange)="set(f.key, $event)">
                @for (o of f.options; track o[0]) {
                  <option [value]="o[0]">{{ o[1] }}</option>
                }
              </select>
            }
            @case ('zone') {
              <select [id]="id" [name]="id" [ngModel]="values()[f.key]" (ngModelChange)="set(f.key, $event)">
                @for (o of f.options; track o[0]) {
                  <option [value]="o[0]">{{ o[1] }}</option>
                }
              </select>
            }
            @case ('number') {
              <input type="number" [id]="id" [name]="id" [min]="f.min ?? null" [max]="f.max ?? null"
                     [ngModel]="values()[f.key]" (ngModelChange)="set(f.key, '' + ($event ?? ''))" />
            }
            @case ('time') {
              <input type="time" [id]="id" [name]="id" [ngModel]="values()[f.key]"
                     (ngModelChange)="set(f.key, $event)" />
            }
            @case ('secret') {
              <input type="password" [id]="id" [name]="id" autocomplete="off" [ngModel]="values()[f.key]"
                     (ngModelChange)="set(f.key, $event)"
                     [placeholder]="f.set ? 'Saved — type a new one to replace it' : ''" />
            }
            @default {
              <input type="text" [id]="id" [name]="id" [ngModel]="values()[f.key]"
                     (ngModelChange)="set(f.key, $event)" [placeholder]="f.default" />
            }
          }
        }
      }
      @if (f.help) {
        <p class="muted small">{{ f.help }}</p>
      }
    }
  `,
})
export class SettingFields {
  readonly fields = input.required<SettingField[]>();
  readonly prefix = input('setting');
  readonly values = model.required<Record<string, string>>();

  protected set(key: string, value: string): void {
    this.values.set({ ...this.values(), [key]: value });
  }
}

/** The form's starting values: what is saved (secrets start empty). */
export function initialValues(fields: SettingField[]): Record<string, string> {
  return Object.fromEntries(fields.map((f) => [f.key, f.kind === 'secret' ? '' : (f.value ?? f.default)]));
}

/** Only what changed; an empty secret means "keep", so it is left out. */
export function changedValues(fields: SettingField[], values: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of fields) {
    const value = values[f.key] ?? '';
    if (f.kind === 'secret' ? value !== '' : value !== (f.value ?? f.default)) out[f.key] = value;
  }
  return out;
}
