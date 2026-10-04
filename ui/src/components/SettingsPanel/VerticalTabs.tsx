/**
 * VerticalTabs - Vertical tab navigation for settings panel
 *
 * Design: jamuru brand (ui.pen Screens/Settings sidebar). Flat near-black
 * surfaces, yellow accent icon on the selected tab.
 */

import { useCallback, useRef, KeyboardEvent, type ReactNode } from "react";
import "./VerticalTabs.css";

export interface Tab {
  id: string;
  label: string;
  /** Optional leading icon (hand-inlined SVG). */
  icon?: ReactNode;
}

export interface VerticalTabsProps {
  /** List of tabs */
  tabs: Tab[];
  /** Currently selected tab ID */
  selectedId: string;
  /** Callback when tab is selected */
  onSelect: (id: string) => void;
}

export function VerticalTabs({ tabs, selectedId, onSelect }: VerticalTabsProps) {
  const tabRefs = useRef<Map<string, HTMLButtonElement>>(new Map());

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      const currentIndex = tabs.findIndex((t) => t.id === selectedId);
      let newIndex = currentIndex;

      switch (e.key) {
        case "ArrowUp":
          e.preventDefault();
          newIndex = currentIndex > 0 ? currentIndex - 1 : tabs.length - 1;
          break;
        case "ArrowDown":
          e.preventDefault();
          newIndex = currentIndex < tabs.length - 1 ? currentIndex + 1 : 0;
          break;
        case "Home":
          e.preventDefault();
          newIndex = 0;
          break;
        case "End":
          e.preventDefault();
          newIndex = tabs.length - 1;
          break;
        default:
          return;
      }

      const newTab = tabs[newIndex];
      if (newTab) {
        onSelect(newTab.id);
        tabRefs.current.get(newTab.id)?.focus();
      }
    },
    [tabs, selectedId, onSelect]
  );

  return (
    <div
      className="vertical-tabs"
      role="tablist"
      aria-orientation="vertical"
      onKeyDown={handleKeyDown}
    >
      {tabs.map((tab) => {
        const isSelected = tab.id === selectedId;
        return (
          <button
            key={tab.id}
            ref={(el) => {
              if (el) {
                tabRefs.current.set(tab.id, el);
              } else {
                tabRefs.current.delete(tab.id);
              }
            }}
            role="tab"
            aria-selected={isSelected}
            aria-controls={`tabpanel-${tab.id}`}
            id={`tab-${tab.id}`}
            tabIndex={isSelected ? 0 : -1}
            className={`vertical-tabs__tab ${isSelected ? "vertical-tabs__tab--selected" : ""}`}
            onClick={() => onSelect(tab.id)}
          >
            {tab.icon && (
              <span className="vertical-tabs__tab-icon" aria-hidden="true">
                {tab.icon}
              </span>
            )}
            <span className="vertical-tabs__tab-label">{tab.label}</span>
          </button>
        );
      })}
    </div>
  );
}

export default VerticalTabs;
