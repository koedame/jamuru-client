/**
 * Main Application
 *
 * Root component for the jamjam P2P audio application.
 * Settings opens in a separate Tauri window. So does the window someone
 * helping works in, which draws the helped app's screen (`HelperScreen`).
 * Diagnostics are integrated into the Settings window's Diagnostics tab.
 */
import { useState, useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { MainScreen } from "./screens/MainScreen";
import { HelperScreen } from "./screens/HelperScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { windowOpenSettings, configGetLanguage } from "./lib/tauri";
import { useWindowEvent } from "./hooks/useWindowEvents";
import { HELP_HASH } from "./lib/helperBackend";

type Screen = "main" | "settings" | "help";

/**
 * The screen a Tauri window was opened for, from its URL hash. Read while
 * choosing the initial state, not in an effect: the main screen connects to
 * the signaling server as soon as it mounts, so rendering it for one frame in
 * the settings window opened a second connection every time.
 */
function screenFromHash(): Screen {
  const hash = window.location.hash;
  if (hash === HELP_HASH) return "help";
  return hash === "#/settings" || hash === "#settings" ? "settings" : "main";
}

function App() {
  const { i18n } = useTranslation();
  const [currentScreen, setCurrentScreen] = useState<Screen>(screenFromHash);

  // Each window's i18next instance starts from its own webview's
  // localStorage/navigator detection, which can be stale or absent (a fresh
  // window, or a language chosen in another window before this one existed).
  // The saved config is the source of truth, so apply it once at startup.
  useEffect(() => {
    configGetLanguage()
      .then((language) => {
        if (language && language !== i18n.language) {
          i18n.changeLanguage(language);
        }
      })
      .catch((e) => console.error("Failed to load the saved language:", e));
  }, [i18n]);

  // The settings window broadcasts this when the user changes the language,
  // so every open window switches immediately - not just the one that
  // changed it. A Tauri event rather than the localStorage `storage` event,
  // because separate webviews don't fire `storage` consistently.
  const handleLanguageChanged = useCallback(
    (language: string) => {
      if (language !== i18n.language) {
        i18n.changeLanguage(language);
      }
    },
    [i18n]
  );
  useWindowEvent<string>("i18n:language-changed", handleLanguageChanged);

  const handleOpenSettings = useCallback(async () => {
    try {
      await windowOpenSettings();
    } catch (e) {
      console.error("Failed to open settings window:", e);
      // Fallback to in-app navigation if Tauri command fails
      setCurrentScreen("settings");
    }
  }, []);

  if (currentScreen === "settings") {
    return <SettingsScreen />;
  }
  if (currentScreen === "help") {
    return <HelperScreen />;
  }

  return <MainScreen onSettingsClick={handleOpenSettings} />;
}

export default App;
