/**
 * Webview side of the diagnostic log file.
 *
 * The bridge to the Rust side is `invoke("log_frontend", ...)`; these tests
 * read what was sent through it.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(),
}));

import { invoke as tauriInvoke, isTauri } from '@tauri-apps/api/core';
import { createRepeatLimiter, formatConsoleArgs, installFrontendLogging } from './logging';
import { invoke } from './invoke';

const backend = vi.mocked(tauriInvoke);

interface SentLine {
  level: string;
  message: string;
}

/** Lines the webview asked the Rust side to write. */
function sentLines(): SentLine[] {
  return backend.mock.calls
    .filter(([cmd]) => cmd === 'log_frontend')
    .map(([, args]) => args as unknown as SentLine);
}

/** Bridge calls are made in a microtask, so let them run. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('installFrontendLogging', () => {
  let uninstall: () => void;

  beforeEach(() => {
    backend.mockReset();
    backend.mockResolvedValue(undefined);
    vi.mocked(isTauri).mockReturnValue(true);
    // Keep the test run's own output clean; the wrapper still forwards to it.
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'log').mockImplementation(() => {});
    uninstall = installFrontendLogging();
  });

  afterEach(() => {
    uninstall();
    vi.restoreAllMocks();
  });

  it('console.error is called, the line reaches the log file at error level', async () => {
    console.error('Failed to load peer name:', new Error('boom'));
    await flush();

    const [line] = sentLines();
    expect(line.level).toBe('error');
    expect(line.message).toContain('Failed to load peer name:');
    expect(line.message).toContain('boom');
  });

  it('console.log, warn and info are called, they keep their levels (log becomes info)', async () => {
    console.log('a');
    console.warn('b');
    console.info('c');
    await flush();

    expect(sentLines()).toEqual([
      { level: 'info', message: 'a' },
      { level: 'warn', message: 'b' },
      { level: 'info', message: 'c' },
    ]);
  });

  it('a promise is rejected and nobody handles it, the reason reaches the log file', async () => {
    const event = new Event('unhandledrejection');
    Object.assign(event, { reason: 'Command plugin:event|listen not allowed by ACL' });
    window.dispatchEvent(event);
    await flush();

    expect(sentLines()).toEqual([
      {
        level: 'error',
        message: 'Unhandled promise rejection: Command plugin:event|listen not allowed by ACL',
      },
    ]);
  });

  it('an error escapes every handler, its message and place reach the log file', async () => {
    window.dispatchEvent(
      new ErrorEvent('error', { message: 'x is not a function', filename: 'app.js', lineno: 12, colno: 3 })
    );
    await flush();

    const [line] = sentLines();
    expect(line.level).toBe('error');
    expect(line.message).toContain('x is not a function at app.js:12:3');
  });

  it('the log command itself fails, console output still works and does not loop', async () => {
    backend.mockRejectedValue('no such command');

    expect(() => console.error('still fine')).not.toThrow();
    await flush();

    expect(backend).toHaveBeenCalledTimes(1);
  });

  it('logging is uninstalled, console output no longer reaches the log file', async () => {
    uninstall();

    console.error('after uninstall');
    await flush();

    expect(sentLines()).toEqual([]);
    uninstall = installFrontendLogging();
  });
});

describe('installFrontendLogging outside a Tauri webview', () => {
  it('the page is not in Tauri, console output is not sent anywhere', async () => {
    backend.mockReset();
    vi.mocked(isTauri).mockReturnValue(false);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const uninstall = installFrontendLogging();

    console.error('in a browser');
    await flush();

    expect(backend).not.toHaveBeenCalled();
    uninstall();
    vi.restoreAllMocks();
  });
});

describe('invoke', () => {
  let uninstall: () => void;

  beforeEach(() => {
    backend.mockReset();
    vi.mocked(isTauri).mockReturnValue(true);
    uninstall = installFrontendLogging();
  });

  afterEach(() => {
    uninstall();
  });

  it('a command fails, its name and error reach the log file and the caller still gets the error', async () => {
    backend.mockImplementation(async (cmd) => {
      if (cmd === 'log_frontend') return undefined;
      throw 'Command signaling_connect not allowed by ACL';
    });

    await expect(invoke('signaling_connect')).rejects.toBe(
      'Command signaling_connect not allowed by ACL'
    );
    await flush();

    const [line] = sentLines();
    expect(line.level).toBe('error');
    expect(line.message).toMatch(
      /^invoke signaling_connect failed \(\d+ms\): Command signaling_connect not allowed by ACL$/
    );
  });

  it('a command with a password argument fails, the argument is not in the log file', async () => {
    backend.mockImplementation(async (cmd) => {
      if (cmd === 'log_frontend') return undefined;
      throw 'Invalid password';
    });

    await expect(
      invoke('signaling_join_room', { roomId: 'r1', password: 'hunter2' })
    ).rejects.toBe('Invalid password');
    await flush();

    expect(JSON.stringify(sentLines())).not.toContain('hunter2');
  });

  it('a command succeeds, the call is logged at debug and the result is returned', async () => {
    backend.mockImplementation(async (cmd) => (cmd === 'log_frontend' ? undefined : 42));

    await expect(invoke('config_get_sample_rate')).resolves.toBe(42);
    await flush();

    const [line] = sentLines();
    expect(line.level).toBe('debug');
    expect(line.message).toMatch(/^invoke config_get_sample_rate ok \(\d+ms\)$/);
  });

  it('a polled command succeeds, nothing is logged', async () => {
    backend.mockImplementation(async (cmd) => (cmd === 'log_frontend' ? undefined : null));

    await invoke('streaming_status');
    await flush();

    expect(sentLines()).toEqual([]);
  });

  it('a polled command keeps failing, only the first few failures are logged', async () => {
    backend.mockImplementation(async (cmd) => {
      if (cmd === 'log_frontend') return undefined;
      throw 'Streaming not started';
    });

    for (let i = 0; i < 50; i++) {
      await invoke('streaming_status').catch(() => {});
    }
    await flush();

    expect(sentLines().length).toBe(3);
  });
});

describe('the lines sent to the log file', () => {
  it('two lines are logged in a row, the second is sent only after the first was written', async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    backend.mockReset();
    vi.spyOn(console, 'info').mockImplementation(() => {});
    const finished: Array<() => void> = [];
    backend.mockImplementation(
      () => new Promise<void>((resolve) => finished.push(resolve))
    );
    const uninstall = installFrontendLogging();

    console.info('first');
    console.info('second');
    await flush();
    expect(sentLines().map((l) => l.message)).toEqual(['first']);

    finished[0]();
    await flush();
    expect(sentLines().map((l) => l.message)).toEqual(['first', 'second']);

    uninstall();
    vi.restoreAllMocks();
  });
});

describe('createRepeatLimiter', () => {
  it('a line repeats within the window, the fourth is held back and counted on the next one after the window', () => {
    let now = 0;
    const limit = createRepeatLimiter(() => now);

    expect([1, 2, 3, 4, 5].map(() => limit('same').write)).toEqual([true, true, true, false, false]);

    now = 10_000;
    expect(limit('same')).toEqual({ write: true, heldBack: 2 });
  });

  it('two different lines repeat, each has its own allowance', () => {
    const limit = createRepeatLimiter(() => 0);
    for (let i = 0; i < 3; i++) limit('a');

    expect(limit('a').write).toBe(false);
    expect(limit('b').write).toBe(true);
  });
});

describe('formatConsoleArgs', () => {
  it('arguments are strings, objects and errors, they are joined into one line', () => {
    const text = formatConsoleArgs(['joined', { room: 'A' }, new Error('boom'), undefined, 7]);

    expect(text).toContain('joined {"room":"A"}');
    expect(text).toContain('Error: boom');
    expect(text).toContain('undefined 7');
  });

  it('an object refers to itself, formatting still returns text', () => {
    const loop: Record<string, unknown> = {};
    loop.self = loop;

    expect(() => formatConsoleArgs([loop])).not.toThrow();
  });
});
