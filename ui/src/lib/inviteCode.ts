/**
 * Invite code format (REQ-CON-103)
 *
 * Shared by the deep-link parser (`deepLink.ts`) and the code input
 * (`ConnectionPanel`), which validated codes with a looser, separate regex
 * before this module existed. Mirrors `parse_invite_url` in
 * `src/network/signaling.rs`: same length, same alphabet.
 */

/** Characters used by invite codes. Excludes 0, O, I, 1, L as confusable. */
export const INVITE_CODE_CHARS = 'ABCDEFGHJKMNPQRSTUVWXYZ23456789';

/** Invite codes are exactly this long. */
export const INVITE_CODE_LENGTH = 6;

/** Whether `code` is a well-formed invite code, regardless of case. */
export function isValidInviteCode(code: string): boolean {
  if (code.length !== INVITE_CODE_LENGTH) {
    return false;
  }
  for (const char of code.toUpperCase()) {
    if (!INVITE_CODE_CHARS.includes(char)) {
      return false;
    }
  }
  return true;
}
