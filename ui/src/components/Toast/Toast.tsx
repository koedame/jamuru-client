import { AlertCircleIcon, CheckIcon, LoaderIcon, TriangleAlertIcon } from "../../lib/icons";
import "./Toast.css";

export type ToastType = "success" | "error" | "info" | "warning";

export interface ToastProps {
  type: ToastType;
  message: string;
}

const ICONS: Record<ToastType, typeof CheckIcon> = {
  success: CheckIcon,
  error: AlertCircleIcon,
  info: LoaderIcon,
  warning: TriangleAlertIcon,
};

/** Single toast card. Stacking/auto-dismiss timing is
 * the caller's responsibility (see components/README.md#toast). */
export function Toast({ type, message }: ToastProps) {
  const Icon = ICONS[type];
  return (
    <div className={`toast toast--${type}`} role={type === "error" ? "alert" : "status"}>
      <span className={`toast__icon${type === "info" ? " toast__icon--spin" : ""}`}>
        <Icon size={16} />
      </span>
      <span className="toast__message">{message}</span>
    </div>
  );
}
