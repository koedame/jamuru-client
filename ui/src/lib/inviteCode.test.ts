import { describe, it, expect } from 'vitest';

import { isValidInviteCode } from './inviteCode';

describe('isValidInviteCode', () => {
  it('accepts a 6-character code drawn from the invite alphabet', () => {
    expect(isValidInviteCode('ABC234')).toBe(true);
  });

  it('accepts lower case, because the input and the deep link both upper-case', () => {
    expect(isValidInviteCode('abc234')).toBe(true);
  });

  it.each([
    ['too short', 'ABC'],
    ['too long', 'ABC2345'],
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
