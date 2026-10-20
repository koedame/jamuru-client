/**
 * AddReactionButton - Single button to add a reaction
 * Opens emoji picker on click
 */
import { useState, useCallback, useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { EmojiPicker } from "./EmojiPicker";
import "./AddReactionButton.css";

export interface AddReactionButtonProps {
  /** Callback when an emoji is selected */
  onSelect?: (emoji: string) => void;
  /** Recently used emojis for the picker */
  recentEmojis?: string[];
  /** Disable the button */
  disabled?: boolean;
}

/** Gap between the trigger and the picker, and minimum gap to the window edge */
const PICKER_GAP = 4;
const PICKER_EDGE_MARGIN = 8;

export function AddReactionButton({
  onSelect,
  recentEmojis = [],
  disabled = false,
}: AddReactionButtonProps) {
  const { t } = useTranslation();
  const [showPicker, setShowPicker] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);

  // The chat column clips its children, so the picker is drawn on the document
  // body and placed against the window instead of the column.
  useLayoutEffect(() => {
    if (!showPicker) return;
    const place = () => {
      const trigger = triggerRef.current;
      const picker = pickerRef.current;
      if (!trigger || !picker) return;
      const anchor = trigger.getBoundingClientRect();
      const width = picker.offsetWidth;
      const height = picker.offsetHeight;
      const maxLeft = window.innerWidth - width - PICKER_EDGE_MARGIN;
      const left = Math.max(PICKER_EDGE_MARGIN, Math.min(anchor.left, maxLeft));
      const above = anchor.top - PICKER_GAP - height;
      const below = anchor.bottom + PICKER_GAP;
      const maxTop = window.innerHeight - height - PICKER_EDGE_MARGIN;
      const top =
        above >= PICKER_EDGE_MARGIN
          ? above
          : Math.max(PICKER_EDGE_MARGIN, Math.min(below, maxTop));
      picker.style.left = `${left}px`;
      picker.style.top = `${top}px`;
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [showPicker]);

  const handleClick = useCallback(() => {
    if (!disabled) {
      setShowPicker((prev) => !prev);
    }
  }, [disabled]);

  const handleSelect = useCallback(
    (emoji: string) => {
      onSelect?.(emoji);
      setShowPicker(false);
    },
    [onSelect]
  );

  const handleClose = useCallback(() => {
    setShowPicker(false);
  }, []);

  return (
    <div className="add-reaction-button">
      <button
        ref={triggerRef}
        type="button"
        className="add-reaction-button__trigger"
        onClick={handleClick}
        disabled={disabled}
        aria-label={t("chat.addReaction")}
        aria-expanded={showPicker}
      >
        <span className="add-reaction-button__icon">☺</span>
      </button>
      {showPicker &&
        createPortal(
          <div ref={pickerRef} className="add-reaction-button__picker">
            <EmojiPicker
              onSelect={handleSelect}
              onClose={handleClose}
              recentEmojis={recentEmojis}
              disabled={disabled}
            />
          </div>,
          document.body
        )}
    </div>
  );
}

export default AddReactionButton;
