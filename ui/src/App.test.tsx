/**
 * Which screen a Tauri window shows is decided by the URL hash it was opened
 * with. The main screen connects to the signaling server as soon as it
 * mounts, so it must never mount in the settings window.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act } from 'react';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import i18n from './i18n';

const mainScreenMounted = vi.fn();

vi.mock('./screens/MainScreen', async () => {
  const React = await import('react');
  return {
    MainScreen: () => {
      React.useEffect(() => {
        mainScreenMounted();
      }, []);
      return <div>main screen</div>;
    },
  };
});

vi.mock('./screens/SettingsScreen', () => ({
  SettingsScreen: () => <div>settings screen</div>,
}));

const configGetLanguage = vi.fn();
const termsGet = vi.fn();
const termsAccept = vi.fn();
vi.mock('./lib/tauri', () => ({
  windowOpenSettings: vi.fn(),
  configGetLanguage: (...args: unknown[]) => configGetLanguage(...args),
  termsGet: (...args: unknown[]) => termsGet(...args),
  termsAccept: (...args: unknown[]) => termsAccept(...args),
}));

// A window's language listener, captured so tests can simulate the settings
// window broadcasting a change without a real second window.
const languageChangedHandlers: Array<(event: { payload: string }) => void> = [];
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((eventName: string, handler: (event: { payload: string }) => void) => {
    if (eventName === 'i18n:language-changed') {
      languageChangedHandlers.push(handler);
    }
    return Promise.resolve(() => {});
  }),
}));

import App from './App';

describe('App', () => {
  beforeEach(() => {
    mainScreenMounted.mockClear();
    configGetLanguage.mockReset();
    configGetLanguage.mockResolvedValue(null);
    termsGet.mockReset();
    termsGet.mockResolvedValue({ version: 1, accepted: true, text: '# 利用規約' });
    termsAccept.mockReset();
    termsAccept.mockResolvedValue(undefined);
    languageChangedHandlers.length = 0;
  });

  afterEach(async () => {
    window.location.hash = '';
    await act(async () => {
      await i18n.changeLanguage('en');
    });
  });

  it('the window is opened with the settings hash, the settings screen shows and the main screen never mounts', () => {
    window.location.hash = '#/settings';

    render(<App />);

    expect(screen.getByText('settings screen')).toBeInTheDocument();
    expect(mainScreenMounted).not.toHaveBeenCalled();
  });

  it('the window is opened without a hash and the terms were agreed to, the main screen shows', async () => {
    render(<App />);

    expect(await screen.findByText('main screen')).toBeInTheDocument();
    // The mount is recorded in an effect, which runs after the element is
    // already in the document.
    await waitFor(() => expect(mainScreenMounted).toHaveBeenCalledTimes(1));
  });

  // The backend holds back everything that reaches the network until the
  // agreement is recorded; the screen must not be the one thing that goes on.
  //
  // Verifies: REQ-TRM-001
  it('the terms were not agreed to, the consent screen shows first and the main screen does not mount', async () => {
    termsGet.mockResolvedValue({ version: 1, accepted: false, text: '# 利用規約' });

    render(<App />);

    expect(await screen.findByTestId('consent-screen')).toBeInTheDocument();
    expect(screen.queryByText('main screen')).not.toBeInTheDocument();
    expect(mainScreenMounted).not.toHaveBeenCalled();
  });

  // Verifies: REQ-TRM-001
  it('the terms are agreed to on the consent screen, the version shown is recorded and the main screen mounts', async () => {
    termsGet.mockResolvedValue({ version: 7, accepted: false, text: '# 利用規約' });

    render(<App />);
    fireEvent.click(await screen.findByTestId('consent-agree'));
    fireEvent.click(screen.getByTestId('consent-adult'));
    fireEvent.click(screen.getByTestId('consent-start'));

    expect(await screen.findByText('main screen')).toBeInTheDocument();
    expect(termsAccept).toHaveBeenCalledWith(7);
  });

  // Verifies: REQ-TRM-002
  it('the agreement cannot be saved, the consent screen stays and says why', async () => {
    termsGet.mockResolvedValue({ version: 1, accepted: false, text: '# 利用規約' });
    termsAccept.mockRejectedValue('disk full');

    render(<App />);
    fireEvent.click(await screen.findByTestId('consent-agree'));
    fireEvent.click(screen.getByTestId('consent-adult'));
    fireEvent.click(screen.getByTestId('consent-start'));

    expect(await screen.findByTestId('consent-error')).toHaveTextContent('disk full');
    expect(mainScreenMounted).not.toHaveBeenCalled();
  });

  // The terms could not be read: the app stays on a screen that does nothing
  // rather than starting without an agreement.
  it('the terms cannot be read, the main screen does not mount', async () => {
    termsGet.mockRejectedValue('no backend');

    render(<App />);

    expect(await screen.findByTestId('consent-load-error')).toBeInTheDocument();
    expect(mainScreenMounted).not.toHaveBeenCalled();
  });

  // Given the user chose a language in a previous session
  // When a window (re)starts
  // Then it shows the language saved in config, not whatever this webview's
  // own localStorage happens to have
  //
  // Verifies: REQ-I18N-104
  it('applies the language saved in config at startup', async () => {
    configGetLanguage.mockResolvedValue('ja');

    render(<App />);

    await waitFor(() => expect(i18n.language).toBe('ja'));
  });

  // Given no language has been saved yet (a fresh install)
  // When a window starts
  // Then it does not force a language - it leaves whatever this window
  // detected on its own (browser/localStorage) alone
  //
  // Verifies: REQ-I18N-101
  it('does not override the detected language when config has none saved', async () => {
    configGetLanguage.mockResolvedValue(null);
    const changeLanguageSpy = vi.spyOn(i18n, 'changeLanguage');

    render(<App />);
    await waitFor(() => expect(configGetLanguage).toHaveBeenCalled());
    // Give the resolved (null) promise's `.then()` a turn to run.
    await act(async () => {
      await Promise.resolve();
    });

    expect(changeLanguageSpy).not.toHaveBeenCalled();
    changeLanguageSpy.mockRestore();
  });

  // Given another window (settings) changed the language and broadcast it
  // When this window is already open
  // Then it switches immediately - before this was fixed, only the window
  // the user changed it in updated; every other open window kept showing
  // the old language until restarted.
  //
  // Verifies: REQ-I18N-102
  it('switches language immediately when another window broadcasts a change', async () => {
    render(<App />);

    await waitFor(() => expect(languageChangedHandlers.length).toBeGreaterThan(0));

    await act(async () => {
      languageChangedHandlers.forEach((handler) => handler({ payload: 'ja' }));
    });

    expect(i18n.language).toBe('ja');
  });
});
