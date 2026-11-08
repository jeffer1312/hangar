import { describe, expect, it } from 'vitest';
import { installCode } from './installCode';
import cases from './installCode.fixtures.json';

describe('installCode', () => {
  it('shows the same code the native app shows for the same install id', () => {
    for (const { install_id, code } of cases) expect(installCode(install_id), install_id).toBe(code);
  });
});
