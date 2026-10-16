/**
 * AboutTab - The update check, and the terms of use, the license and the pages published for users
 *
 * The legal rows themselves live in LegalSection (ADR-072); the update check
 * in UpdateSection (ADR-082), shown when it is given.
 */

import { LegalSection, type LegalSectionProps } from "../../Terms/LegalSection";
import { UpdateSection, type UpdateSectionProps } from "./UpdateSection";
import "./TabContent.css";

export interface AboutTabProps extends LegalSectionProps {
  /** The update section; not shown when absent */
  update?: UpdateSectionProps;
}

export function AboutTab({ update, ...legal }: AboutTabProps) {
  return (
    <div className="tab-content">
      {update && <UpdateSection {...update} />}
      <LegalSection {...legal} />
    </div>
  );
}

export default AboutTab;
