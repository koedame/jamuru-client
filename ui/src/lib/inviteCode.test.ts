import { describe, it, expect } from 'vitest';

import { isValidInviteCode } from './inviteCode';

describe('isValidInviteCode', () => {
  it('accepts a 9-character code drawn from the invite alphabet', () => {
    expect(isValidInviteCode('ABC234XYZ')).toBe(true);
  });

  it('rejects a 6-character code, which no room has any more', () => {
    expect(isValidInviteCode('HJK567')).toBe(false);
  });

  it('accepts lower case, because the input and the deep link both upper-case', () => {
    expect(isValidInviteCode('abc234xyz')).toBe(true);
  });

  it.each([
    ['too short', 'ABC'],
    ['a length between six and nine', 'ABC2345'],
    ['8 characters, which is a room ID', 'ABC234XY'],
    ['too long', 'ABC234XYZ2'],
    ['a confusable 0', 'ABC0EFGHJ'],
    ['a confusable O', 'ABCOEFGHJ'],
    ['a confusable I', 'ABCIEFGHJ'],
    ['a confusable 1', 'ABC1EFGHJ'],
    ['a confusable L', 'ABCLEFGHJ'],
    ['empty', ''],
  ])('rejects %s', (_label, code) => {
    expect(isValidInviteCode(code)).toBe(false);
  });
});
