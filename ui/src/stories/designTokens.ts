/**
 * The design tokens, read from `styles/tokens.css` itself. The Foundation
 * stories draw from this list, so a token added to the CSS shows up in
 * Storybook without a second place to edit.
 */
import tokensCss from "../styles/tokens.css?raw";

export type TokenGroup =
  | "color"
  | "typography"
  | "spacing"
  | "radius"
  | "shadow"
  | "motion"
  | "layer"
  | "layout";

export interface Token {
  /** `--color-bg-primary` */
  name: string;
  /** The value as written in tokens.css (an alias keeps its `var(...)`) */
  declared: string;
  group: TokenGroup;
}

const GROUP_BY_PREFIX: ReadonlyArray<readonly [string, TokenGroup]> = [
  ["--color-", "color"],
  ["--font-", "typography"],
  ["--line-height-", "typography"],
  ["--letter-spacing-", "typography"],
  ["--space-", "spacing"],
  ["--gap-", "spacing"],
  ["--padding-", "spacing"],
  ["--radius-", "radius"],
  ["--shadow-", "shadow"],
  ["--duration-", "motion"],
  ["--ease-", "motion"],
  ["--transition-", "motion"],
  ["--z-", "layer"],
  ["--breakpoint-", "layout"],
  ["--container-", "layout"],
];

/** The group a token belongs to; null when no group takes its prefix */
export function groupOf(name: string): TokenGroup | null {
  return GROUP_BY_PREFIX.find(([prefix]) => name.startsWith(prefix))?.[1] ?? null;
}

/**
 * Every custom property tokens.css declares, in file order. A name declared
 * again (the light theme, reduced motion) keeps its first declaration, the
 * default dark one. A token no group takes is listed in `ungrouped`, so a
 * test can fail on it.
 */
export function parseTokens(css: string): { tokens: Token[]; ungrouped: string[] } {
  const seen = new Set<string>();
  const tokens: Token[] = [];
  const ungrouped: string[] = [];
  const body = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of body.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    const name = match[1];
    if (seen.has(name)) continue;
    seen.add(name);
    const group = groupOf(name);
    if (group === null) ungrouped.push(name);
    else tokens.push({ name, declared: match[2].replace(/\s+/g, " ").trim(), group });
  }
  return { tokens, ungrouped };
}

export const TOKENS = parseTokens(tokensCss);

export function tokensOf(group: TokenGroup): Token[] {
  return TOKENS.tokens.filter((t) => t.group === group);
}
