import { describe, expect, it } from 'vitest';

import { SettingField } from '../api/models';
import { changedValues, initialValues, isShown, startsGroup } from './setting-fields';

const FIELDS: SettingField[] = [
  { key: 'time', label: 'Time', help: '', kind: 'time', default: '07:30', optional: false, value: '06:00' },
  { key: 'message', label: 'Message', help: '', kind: 'long_text', default: 'Hi', optional: false },
  { key: 'key', label: 'Key', help: '', kind: 'secret', default: '', optional: false, set: true },
];

describe('setting fields', () => {
  it('start from saved values, defaults, and empty secrets', () => {
    expect(initialValues(FIELDS)).toEqual({ time: '06:00', message: 'Hi', key: '' });
  });

  it('send only what changed, and a secret only when typed', () => {
    const values = initialValues(FIELDS);
    expect(changedValues(FIELDS, values)).toEqual({});
    expect(changedValues(FIELDS, { ...values, time: '07:15', key: 'sk-new' })).toEqual({
      time: '07:15',
      key: 'sk-new',
    });
  });

  it('draw a group heading once over its first field', () => {
    const toggle = (key: string, group?: string): SettingField => ({
      key, label: key, help: '', kind: 'toggle', default: 'no', optional: false, group,
    });
    const fields = [toggle('a'), toggle('b', 'Post about'), toggle('c', 'Post about'), toggle('d')];
    expect(fields.map((_, i) => startsGroup(fields, i))).toEqual([false, true, false, false]);
  });

  it("hide a strategy's fields until it is chosen", () => {
    const field: SettingField = {
      key: 'band_buy', label: 'Buy at', help: '', kind: 'number', default: '10', optional: false,
      shown_when: { key: 'strategy', values: ['band'] },
    };
    expect(isShown(field, { strategy: 'band' })).toBe(true);
    expect(isShown(field, { strategy: 'lotto' })).toBe(false);
    expect(isShown(FIELDS[0], {})).toBe(true);
  });
});
