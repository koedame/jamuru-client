import { describe, it, expect } from "vitest";
import tokensCss from "../styles/tokens.css?raw";
import { groupOf, parseTokens } from "./designTokens";

describe("デザイントークンの一覧", () => {
  it("tokens.css に宣言されたトークンがすべていずれかのグループに入っていること", () => {
    const { tokens, ungrouped } = parseTokens(tokensCss);

    expect(ungrouped).toEqual([]);
    expect(tokens.length).toBeGreaterThan(100);
  });

  it("同じ名前を後から宣言し直しているとき、最初の宣言を採ること", () => {
    const { tokens } = parseTokens(`
      :root { --color-a: #111; }
      :root[data-theme="light"] { --color-a: #fff; }
    `);

    expect(tokens).toEqual([{ name: "--color-a", declared: "#111", group: "color" }]);
  });

  it("コメントの中のトークンは数えないこと", () => {
    const { tokens } = parseTokens(`/* --color-x: red; */ :root { --space-sm: 8px; }`);

    expect(tokens.map((t) => t.name)).toEqual(["--space-sm"]);
  });

  it("どのグループにも入らない名前のとき、ungrouped に出ること", () => {
    expect(groupOf("--mystery-token")).toBeNull();
    expect(parseTokens(":root { --mystery-token: 1; }").ungrouped).toEqual(["--mystery-token"]);
  });

  it("エイリアスの値は var(...) のまま持つこと", () => {
    const { tokens } = parseTokens(":root { --gap-sm: var(--space-sm); }");

    expect(tokens[0].declared).toBe("var(--space-sm)");
  });
});
