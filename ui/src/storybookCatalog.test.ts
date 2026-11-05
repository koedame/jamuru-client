/**
 * Storybook is where the design lives (the design file `ui.pen` is gone), so a
 * component or screen that is not in it is a gap in the design. These tests
 * stop that from happening unnoticed.
 */
import { describe, it, expect } from "vitest";

const sources = import.meta.glob("./{components,screens}/**/*.tsx", { query: "?raw", import: "default" });
const stories = import.meta.glob("./**/*.stories.tsx", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

/** Components with no stories file of their own, and where they are seen instead */
const SEEN_ELSEWHERE: Record<string, string> = {
  "./components/ChatPanel/ChatPanelAdapter.tsx": "ChatPanel を backend に繋ぐだけの層。見た目は Windows/Chat と Screens/Main",
  "./components/SettingsPanel/SettingsPanelAdapter.tsx":
    "SettingsPanel を backend に繋ぐだけの層。見た目は Windows/Settings と Screens/Settings",
  "./components/Terms/ConsentGate.tsx": "規約を読んで ConsentScreen か子を出すだけの層。見た目は Screens/Consent",
  "./components/SettingsHelp/SettingsHelpMirror.tsx":
    "SidePanel に SettingsHelpPanel を入れるだけの層。見た目は Components/Common/SidePanel と Components/Settings/Help/SettingsHelpPanel",
};

/** A file that is a component: `Name.tsx`, not a test, a story or a hook */
function isComponentFile(path: string): boolean {
  const name = path.split("/").pop()!;
  return /^[A-Z][A-Za-z0-9]*\.tsx$/.test(name) && !/\.(test|stories)\.tsx$/.test(name);
}

describe("Storybook のカタログ", () => {
  it("components と screens の部品・画面がすべて自分の stories を持つか、別の場所で見えると書かれていること", () => {
    const missing = Object.keys(sources)
      .filter(isComponentFile)
      .filter((path) => !(path in SEEN_ELSEWHERE))
      .filter((path) => !(path.replace(/\.tsx$/, ".stories.tsx") in stories));

    expect(missing).toEqual([]);
  });

  it("別の場所で見えると書いた部品が、実在し、stories を持たないこと", () => {
    const stale = Object.keys(SEEN_ELSEWHERE).filter(
      (path) => !(path in sources) || path.replace(/\.tsx$/, ".stories.tsx") in stories
    );

    expect(stale).toEqual([]);
  });

  it("stories の title が Foundation / Components / Screens / Windows のどれかで始まり、重複しないこと", () => {
    const titles = Object.entries(stories).map(([path, source]) => {
      const title = /const meta = \{\s*title: "([^"]+)"/.exec(source)?.[1];
      expect(title, `${path} に meta の title がある`).toBeDefined();
      return title!;
    });

    expect(titles.filter((t) => !/^(Foundation|Components|Screens|Windows)\//.test(t))).toEqual([]);
    expect(titles.filter((t, i) => titles.indexOf(t) !== i)).toEqual([]);
  });

  it("Components の stories が、グループ名と部品名の 3 階層で並んでいること", () => {
    const titles = Object.values(stories)
      .map((source) => /const meta = \{\s*title: "([^"]+)"/.exec(source)?.[1] ?? "")
      .filter((t) => t.startsWith("Components/"));

    expect(titles.filter((t) => t.split("/").length < 3)).toEqual([]);
  });
});
