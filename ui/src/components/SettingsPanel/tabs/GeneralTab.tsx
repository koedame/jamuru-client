/**
 * GeneralTab - General settings (language)
 *
 * Design: jamuru is a dark-only brand (see tokens.css), so the theme selector
 * from the previous design is intentionally omitted; only language remains.
 */

import { useTranslation } from "react-i18next";
import { FormField } from "../FormField";
import { Select } from "../Select";
import { Input } from "../Input";
import { LegalSection, type LegalSectionProps } from "../../Terms/LegalSection";
import "./TabContent.css";

export type Language = "ja" | "en";

export interface GeneralTabProps {
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
  /** Opens the terms of use, the license and the published pages. The section is left out when absent */
  legal?: LegalSectionProps;
}

export function GeneralTab({
  language,
  onLanguageChange,
  serverUrl,
  effectiveServerUrl,
  onServerUrlChange,
  legal,
}: GeneralTabProps) {
  const { t } = useTranslation();

  const languageOptions = [
    { value: "ja", label: "日本語" },
    { value: "en", label: "English" },
  ];

  const isCustom = serverUrl.trim().length > 0;

  return (
    <div className="tab-content">
      <h2 className="tab-content__title">{t("settings.general.title", "General")}</h2>

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

      {legal && <LegalSection {...legal} />}
    </div>
  );
}

export default GeneralTab;
