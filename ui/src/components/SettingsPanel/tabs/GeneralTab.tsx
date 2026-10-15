/**
 * GeneralTab - Settings about the user and the app as a whole: profile
 * (display name), display (language) and connection (signaling server)
 *
 * Design: jamuru is a dark-only brand (see tokens.css), so the theme selector
 * from the previous design is intentionally omitted.
 */

import { useTranslation } from "react-i18next";
import { FormField } from "../FormField";
import { Select } from "../Select";
import { Input } from "../Input";
import "./TabContent.css";

export type Language = "ja" | "en";

export interface GeneralTabProps {
  /** Current display name */
  displayName: string;
  /** Display name error */
  displayNameError?: string;
  /** Display name change handler */
  onDisplayNameChange: (name: string) => void;
  /** Current language */
  language: Language;
  /** Language change handler */
  onLanguageChange: (language: Language) => void;
  /** Signaling server override as stored in config (empty = use the build default) */
  serverUrl: string;
  /** The URL the app will actually dial right now (the override, or the build default) */
  effectiveServerUrl: string;
  /** Server URL change handler. Called with an empty string to clear the override. */
  onServerUrlChange: (url: string) => void;
}

export function GeneralTab({
  displayName,
  displayNameError,
  onDisplayNameChange,
  language,
  onLanguageChange,
  serverUrl,
  effectiveServerUrl,
  onServerUrlChange,
}: GeneralTabProps) {
  const { t } = useTranslation();

  const languageOptions = [
    { value: "ja", label: "日本語" },
    { value: "en", label: "English" },
  ];

  const isCustom = serverUrl.trim().length > 0;

  return (
    <div className="tab-content">
      <h2 className="tab-content__title">{t("settings.profile.title", "Profile")}</h2>

      <FormField
        label={t("settings.profile.name", "Display Name")}
        htmlFor="display-name"
        hint={t("settings.profile.nameHint", "This name is shown to other participants")}
        error={displayNameError}
      >
        <Input
          id="display-name"
          type="text"
          value={displayName}
          onChange={onDisplayNameChange}
          placeholder={t("settings.profile.namePlaceholder")}
          maxLength={32}
          hasError={!!displayNameError}
        />
      </FormField>

      <div className="tab-content__divider" />

      <h2 className="tab-content__title">{t("settings.general.display", "Display")}</h2>

      <FormField
        label={t("settings.display.language", "Language")}
        htmlFor="language-select"
      >
        <Select
          id="language-select"
          options={languageOptions}
          value={language}
          onChange={(value) => onLanguageChange(value as Language)}
        />
      </FormField>

      <div className="tab-content__divider" />

      <h2 className="tab-content__title">{t("settings.general.connection", "Connection")}</h2>

      <FormField
        label={t("signaling.server.label")}
        htmlFor="server-url-input"
        hint={
          isCustom
            ? t("settings.general.serverUrl.customHint", { url: effectiveServerUrl })
            : t("settings.general.serverUrl.defaultHint", { url: effectiveServerUrl })
        }
      >
        <Input
          id="server-url-input"
          value={serverUrl}
          placeholder={effectiveServerUrl}
          onChange={onServerUrlChange}
          spellCheck={false}
          autoComplete="off"
        />
      </FormField>
    </div>
  );
}

export default GeneralTab;
