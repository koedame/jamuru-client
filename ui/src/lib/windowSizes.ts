/**
 * Single source of truth for the main window's two logical-pixel sizes,
 * the join screen (600x700) and the connected screen (1134 wide, height kept
 * at 700 so only the width changes on connect).
 *
 * `src-tauri/tauri.conf.json`'s `windows[0].width/height` (the window's
 * initial size before any React code runs) cannot import this file — keep
 * it in sync with JOIN_WINDOW_SIZE by hand.
 */
export const JOIN_WINDOW_SIZE = { width: 600, height: 700 } as const;
export const MIXER_WINDOW_SIZE = { width: 1134, height: 700 } as const;

/**
 * Minimum sizes enforced at runtime alongside each target size above.
 * JOIN_MIN_SIZE matches tauri.conf.json's static `minWidth`/`minHeight`
 * (the window's floor before any resize happens). MIXER_MIN_SIZE is the
 * connected 3-column layout's real floor: a 240px room sidebar + 280px
 * chat column leaves only 520px for chrome/dividers, so the mixer needs
 * enough width left over to show at least two 100px channel strips
 * without clipping (room-sidebar.css / MainScreen.css).
 */
export const JOIN_MIN_SIZE = { width: 600, height: 500 } as const;
export const MIXER_MIN_SIZE = { width: 800, height: 500 } as const;
