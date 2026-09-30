import { describe, it, expect } from 'vitest';

import { isValidInviteCode } from './inviteCode';

describe('isValidInviteCode', () => {
  it('accepts a 9-character code drawn from the invite alphabet', () => {
    expect(isValidInviteCode('ABC234XYZ')).toBe(true);
  });

  it('accepts a 6-character code, which a deployment fixes for its own rooms', () => {
    expect(isValidInviteCode('ABC234')).toBe(true);
  });

  it('accepts lower case, because the input and the deep link both upper-case', () => {
    expect(isValidInviteCode('abc234xyz')).toBe(true);
  });

  it.each([
    ['too short', 'ABC'],
    ['a length between the two accepted ones', 'ABC2345'],
    ['8 characters, which is a room ID', 'ABC234XY'],
    ['too long', 'ABC234XYZ2'],
    ['a confusable 0', 'ABC0EF'],
    ['a confusable O', 'ABCOEF'],
    ['a confusable I', 'ABCIEF'],
    ['a confusable 1', 'ABC1EF'],
    ['a confusable L', 'ABCLEF'],
    ['empty', ''],
  ])('rejects %s', (_label, code) => {
    expect(isValidInviteCode(code)).toBe(false);
  });
});
