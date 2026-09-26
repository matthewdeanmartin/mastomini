import { describe, expect, it } from 'vitest';

import { SettingField } from '../api/models';
import { changedValues, initialValues } from './setting-fields';

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
});
