/**
 * SettingsScreen - Settings window using Storybook SettingsPanel
 *
 * Reuses the SettingsPanelAdapter component for consistent UI between
 * Storybook and Tauri GUI.
 * Diagnostics are integrated into the Diagnostics tab within SettingsPanel.
 *
 * The window opens on the tab its URL names (`#/settings/about`), and is told
 * to show another one when the menu asks for a tab while it is already open.
 * The menu's "Check for Updates" opens it on the About tab with the check
 * asked for: `#/settings/about/check-update`, or an event to an open window.
 */

import { useCallback, useState } from "react";
import { SettingsPanelAdapter, type SettingsTabId } from "../components/SettingsPanel";
import { useWindowEvent } from "../hooks/useWindowEvents";

/** Sent by the app to an open settings window to switch its tab (`windows.rs`). */
export const SETTINGS_SELECT_TAB = "settings:select-tab";

/** Sent by the app to an open settings window to look for a newer release (`windows.rs`). */
export const SETTINGS_CHECK_UPDATE = "settings:check-update";

const TABS: readonly SettingsTabId[] = ["general", "devices", "privacy", "diagnostics", "about"];

function isTab(value: string | undefined): value is SettingsTabId {
  return TABS.includes(value as SettingsTabId);
}

/** The tab a settings URL hash names, or null for `#/settings` alone. */
export function settingsTabFromHash(hash: string): SettingsTabId | null {
  const tab = hash.split("/")[2];
  return isTab(tab) ? tab : null;
}

/** Whether a settings URL hash asks for the update check as the window opens. */
export function settingsChecksUpdateFromHash(hash: string): boolean {
  return hash.split("/")[3] === "check-update";
}

const addRequest = (requests: number) => requests + 1;

export function SettingsScreen() {
  const [tabRequest, setTabRequest] = useState<{ tab: SettingsTabId } | null>(null);
  const [updateCheckRequest, setUpdateCheckRequest] = useState(
    settingsChecksUpdateFromHash(window.location.hash) ? 1 : 0
  );
  const handleCheckUpdate = useCallback(() => setUpdateCheckRequest(addRequest), []);
  useWindowEvent<void>(SETTINGS_CHECK_UPDATE, handleCheckUpdate);
  const handleSelectTab = useCallback((tab: string) => {
    if (isTab(tab)) setTabRequest({ tab });
  }, []);
  useWindowEvent<string>(SETTINGS_SELECT_TAB, handleSelectTab);

  return (
    <SettingsPanelAdapter
      initialTab={settingsTabFromHash(window.location.hash) ?? "general"}
      tabRequest={tabRequest}
      updateCheckRequest={updateCheckRequest}
    />
  );
}

export default SettingsScreen;
