/**
 * PresenceMarks - the frames that show where the other person of a settings
 * help is looking (REQ-RMT-034). One frame for the part under their pointer,
 * with a dot at the place in it, and one for the part they have focused. The
 * frames are drawn over the screen and take no clicks.
 */
import "./PresenceMarks.css";

export interface PresenceMark {
  /** `hover` or `focus` */
  kind: "hover" | "focus";
  /** The part's place on the screen, in pixels */
  left: number;
  top: number;
  width: number;
  height: number;
  /** For a hover: the place in the part the pointer is at, 0 to 1 */
  at?: { x: number; y: number };
}

export interface PresenceMarksProps {
  marks: PresenceMark[];
  /** Whose frames these are, as a small label */
  name: string;
}

export function PresenceMarks({ marks, name }: PresenceMarksProps) {
  if (marks.length === 0) return null;
  return (
    <div className="presence-marks" aria-hidden="true" data-testid="presence-marks">
      {marks.map((mark, index) => (
        <div
          key={`${mark.kind}-${index}`}
          className={`presence-mark presence-mark--${mark.kind}`}
          data-testid={`presence-mark-${mark.kind}`}
          style={{ left: mark.left, top: mark.top, width: mark.width, height: mark.height }}
        >
          <span className="presence-mark__name">{name}</span>
          {mark.at && (
            <span
              className="presence-mark__dot"
              style={{ left: `${mark.at.x * 100}%`, top: `${mark.at.y * 100}%` }}
            />
          )}
        </div>
      ))}
    </div>
  );
}

export default PresenceMarks;
