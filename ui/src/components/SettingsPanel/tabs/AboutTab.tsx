/**
 * AboutTab - The terms of use, the license and the pages published for users
 *
 * The rows themselves live in LegalSection (ADR-072).
 */

import { LegalSection, type LegalSectionProps } from "../../Terms/LegalSection";
import "./TabContent.css";

export type AboutTabProps = LegalSectionProps;

export function AboutTab(props: AboutTabProps) {
  return (
    <div className="tab-content">
      <LegalSection {...props} />
    </div>
  );
}

export default AboutTab;
