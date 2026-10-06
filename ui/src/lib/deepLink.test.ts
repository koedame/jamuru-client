/**
 * Invite link parsing tests
 *
 * This parser has to agree with `parse_invite_url` in
 * `src/network/signaling.rs`. The cases below are deliberately the same ones
 * `test_invite_url_round_trips` uses, so a change to one side that is not
 * mirrored on the other shows up here.
 */

import { describe, it, expect, vi } from 'vitest';

// The plugin talks to the Tauri runtime, which does not exist under vitest.
vi.mock('@tauri-apps/plugin-deep-link', () => ({
  getCurrent: vi.fn(async () => null),
  onOpenUrl: vi.fn(async () => () => {}),
}));

import { parseInviteUrl, registerInviteLinkHandler } from './deepLink';
import { getCurrent, onOpenUrl } from '@tauri-apps/plugin-deep-link';

describe('parseInviteUrl', () => {
  // Verifies: REQ-CON-103
  it('extracts the code from a well-formed link', () => {
    expect(parseInviteUrl('jamuru://join/ABC234XYZ')).toBe('ABC234XYZ');
  });

  it('upper-cases, because mail and chat clients lower-case links', () => {
    expect(parseInviteUrl('jamuru://join/abc234xyz')).toBe('ABC234XYZ');
  });

  it('tolerates a query string and a trailing slash', () => {
    expect(parseInviteUrl('jamuru://join/ABC234XYZ?from=chat')).toBe('ABC234XYZ');
    expect(parseInviteUrl('jamuru://join/ABC234XYZ/')).toBe('ABC234XYZ');
    expect(parseInviteUrl('  jamuru://join/ABC234XYZ  ')).toBe('ABC234XYZ');
  });

  // Verifies: REQ-CON-103
  it.each([
    ['wrong scheme', 'https://example.com/join/ABC234XYZ'],
    ['wrong action', 'jamuru://leave/ABC234XYZ'],
    ['too short', 'jamuru://join/ABC'],
    ['too long', 'jamuru://join/ABC2345'],
    ['excluded confusable characters', 'jamuru://join/ABC01I'],
    ['no code', 'jamuru://join/'],
    ['deeper path', 'jamuru://join/ABC234XYZ/extra'],
    ['empty', ''],
  ])('rejects %s rather than guessing', (_label, url) => {
    expect(parseInviteUrl(url)).toBeNull();
  });

  it('accepts every character of the invite alphabet', () => {
    // The alphabet excludes 0, O, I, 1 and L as confusable, so a code built
    // from it must parse while one containing an excluded character must not.
    expect(parseInviteUrl('jamuru://join/ZYXW98765')).toBe('ZYXW98765');
    expect(parseInviteUrl('jamuru://join/ZYXW9876O')).toBeNull();
  });
});

describe('registerInviteLinkHandler', () => {
  it('handles a link that started the app before listening for more', async () => {
    vi.mocked(getCurrent).mockResolvedValueOnce(['jamuru://join/ABC234XYZ']);
    const onCode = vi.fn();
    const onInvalidLink = vi.fn();

    await registerInviteLinkHandler(onCode, onInvalidLink);

    expect(onCode).toHaveBeenCalledWith('ABC234XYZ');
    expect(onInvalidLink).not.toHaveBeenCalled();
    expect(onOpenUrl).toHaveBeenCalled();
  });

  it('ignores a launch URL that is not an invite link', async () => {
    vi.mocked(getCurrent).mockResolvedValueOnce(['jamuru://something-else']);
    const onCode = vi.fn();
    const onInvalidLink = vi.fn();

    await registerInviteLinkHandler(onCode, onInvalidLink);

    expect(onCode).not.toHaveBeenCalled();
    expect(onInvalidLink).not.toHaveBeenCalled();
  });

  it('reports a launch URL with the invite scheme but a malformed code', async () => {
    vi.mocked(getCurrent).mockResolvedValueOnce(['jamuru://join/ABC-123']);
    const onCode = vi.fn();
    const onInvalidLink = vi.fn();

    await registerInviteLinkHandler(onCode, onInvalidLink);

    expect(onCode).not.toHaveBeenCalled();
    expect(onInvalidLink).toHaveBeenCalledTimes(1);
  });

  it('logs a malformed invite link, the link itself stays out of the log', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.mocked(getCurrent).mockResolvedValueOnce(['jamuru://join/ABC-123']);

    await registerInviteLinkHandler(vi.fn(), vi.fn());

    expect(warn).toHaveBeenCalled();
    expect(warn.mock.calls.flat().join(' ')).not.toContain('ABC-123');
    warn.mockRestore();
  });

  it('joins only the first invite link in a batch', async () => {
    vi.mocked(getCurrent).mockResolvedValueOnce([
      'jamuru://join/ABC234XYZ',
      'jamuru://join/ZYXW98765',
    ]);
    const onCode = vi.fn();
    const onInvalidLink = vi.fn();

    await registerInviteLinkHandler(onCode, onInvalidLink);

    expect(onCode).toHaveBeenCalledTimes(1);
    expect(onCode).toHaveBeenCalledWith('ABC234XYZ');
    expect(onInvalidLink).not.toHaveBeenCalled();
  });

  it('still registers a listener when reading the launch URL fails', async () => {
    // Deep links are a convenience; failing to read one must not stop the app.
    vi.mocked(getCurrent).mockRejectedValueOnce(new Error('no runtime'));
    const onCode = vi.fn();
    const onInvalidLink = vi.fn();

    await expect(
      registerInviteLinkHandler(onCode, onInvalidLink)
    ).resolves.toBeInstanceOf(Function);
    expect(onOpenUrl).toHaveBeenCalled();
  });
});
