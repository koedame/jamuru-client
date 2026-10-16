/**
 * SettingsScreen - Settings window using Storybook SettingsPanel
 *
 * Reuses the SettingsPanelAdapter component for consistent UI between
 * Storybook and Tauri GUI.
 * Diagnostics are integrated into the Diagnostics tab within SettingsPanel.
 *
 * The window opens on the tab its URL names (`#/settings/about`), and is told
 * to show another one when the menu asks for a tab while it is already open.
 */

import { useCallback, useState } from "react";
import { SettingsPanelAdapter, type SettingsTabId } from "../components/SettingsPanel";
import { useWindowEvent } from "../hooks/useWindowEvents";

/** Sent by the app to an open settings window to switch its tab (`windows.rs`). */
export const SETTINGS_SELECT_TAB = "settings:select-tab";

const TABS: readonly SettingsTabId[] = ["general", "devices", "privacy", "diagnostics", "about"];

function isTab(value: string | undefined): value is SettingsTabId {
  return TABS.includes(value as SettingsTabId);
}

/** The tab a settings URL hash names, or null for `#/settings` alone. */
export function settingsTabFromHash(hash: string): SettingsTabId | null {
  const tab = hash.split("/")[2];
  return isTab(tab) ? tab : null;
}

export function SettingsScreen() {
  const [tabRequest, setTabRequest] = useState<{ tab: SettingsTabId } | null>(null);
  const handleSelectTab = useCallback((tab: string) => {
    if (isTab(tab)) setTabRequest({ tab });
  }, []);
  useWindowEvent<string>(SETTINGS_SELECT_TAB, handleSelectTab);

  return (
    <SettingsPanelAdapter
      initialTab={settingsTabFromHash(window.location.hash) ?? "general"}
      tabRequest={tabRequest}
    />
  );
}

export default SettingsScreen;
