import { useTranslation } from "react-i18next";
import "./Terms.css";

export interface LegalSectionProps {
  onOpenTerms: () => void;
  onOpenLicense: () => void;
  /** Opens the privacy and security page in the browser */
  onOpenPrivacy: () => void;
  /** Opens the announcements page in the browser */
  onOpenAnnouncements: () => void;
}

/** The settings screen's rows that open the terms of use, the license and the pages published for users. */
export function LegalSection({
  onOpenTerms,
  onOpenLicense,
  onOpenPrivacy,
  onOpenAnnouncements,
}: LegalSectionProps) {
  const { t } = useTranslation();
  const rows = [
    { id: "terms", label: t("settings.legal.terms"), onClick: onOpenTerms },
    { id: "license", label: t("settings.legal.license"), onClick: onOpenLicense },
    { id: "privacy", label: t("settings.legal.privacy"), onClick: onOpenPrivacy },
    { id: "announcements", label: t("settings.legal.announcements"), onClick: onOpenAnnouncements },
  ];
  return (
    <section className="legal" data-testid="settings-legal">
      <h3 className="legal__title">{t("settings.legal.title")}</h3>
      {rows.map((row) => (
        <button
          key={row.id}
          type="button"
          className="legal__row"
          onClick={row.onClick}
          data-testid={`settings-legal-${row.id}`}
        >
          {row.label}
        </button>
      ))}
    </section>
  );
}
