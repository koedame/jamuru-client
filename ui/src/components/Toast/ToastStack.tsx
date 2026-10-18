import type { ReactNode } from "react";
import "./ToastStack.css";

/** Where notifications appear: the top centre of the window, over the
 * content. Toasts here never take space from the screen below, so a toast
 * coming and going cannot move the layout. */
export function ToastStack({ children }: { children: ReactNode }) {
  return <div className="toast-stack">{children}</div>;
}
