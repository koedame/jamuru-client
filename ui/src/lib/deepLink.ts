/**
 * Invite link handling (REQ-CON-103)
 *
 * The OS hands `jamuru://join/<code>` to the app via the deep-link plugin. The
 * scheme is declared in `src-tauri/tauri.conf.json` under `plugins.deep-link`.
 *
 * Parsing mirrors `parse_invite_url` in `src/network/signaling.rs`: same scheme,
 * same action, same alphabet and lengths (`./inviteCode`). Both reject rather
 * than guess, so a malformed link fails here with a clear cause instead of
 * reaching the server as "room not found".
 */

import { getCurrent, onOpenUrl } from '@tauri-apps/plugin-deep-link';
import { isValidInviteCode } from './inviteCode';

const INVITE_URL_PREFIX = 'jamuru://join/';

/**
 * Extract the invite code from an invite URL, or null if it is not one.
 *
 * Upper-cases first, so a link that was lower-cased in transit still works.
 * Tolerates a trailing slash and a query string, but not a deeper path.
 */
export function parseInviteUrl(url: string): string | null {
  const trimmed = url.trim();
  if (!trimmed.startsWith(INVITE_URL_PREFIX)) {
    return null;
  }

  const rest = trimmed.slice(INVITE_URL_PREFIX.length);
  const code = rest.split(/[?#]/)[0].replace(/\/+$/, '');
  if (code.includes('/')) {
    return null;
  }

  const upper = code.toUpperCase();
  return isValidInviteCode(upper) ? upper : null;
}

/**
 * Call `onCode` for an invite link, both at launch and while running. Calls
 * `onInvalidLink` instead when a URL has the invite scheme and action but a
 * malformed code, so the caller can tell the user rather than dropping it
 * silently.
 *
 * Returns a cleanup function. Launch URLs are checked first, because a cold
 * start from a clicked link delivers the URL before any listener exists.
 */
export async function registerInviteLinkHandler(
  onCode: (code: string) => void,
  onInvalidLink: () => void
): Promise<() => void> {
  const handleUrls = (urls: string[] | null) => {
    for (const url of urls ?? []) {
      const code = parseInviteUrl(url);
      if (code) {
        // The code itself stays out of the log: it lets whoever reads the file join the room.
        console.info('Received an invite link');
        onCode(code);
        // One link opens one room; ignore anything else in the same batch.
        return;
      }
      if (url.trim().startsWith(INVITE_URL_PREFIX)) {
        // Neither does a malformed one: it is most often a real code with a typo.
        console.warn('Ignoring an invite link with a malformed code');
        onInvalidLink();
        return;
      }
      console.warn('Ignoring a URL that is not an invite link');
    }
  };

  // A link that started the app is waiting here rather than in the listener.
  try {
    handleUrls(await getCurrent());
  } catch (e) {
    // Not fatal: the app runs without deep links, they are a convenience.
    console.warn('Could not read the launch URL:', e);
  }

  return onOpenUrl(handleUrls);
}
