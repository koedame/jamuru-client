/**
 * Tests for splitLinks: which parts of a chat message become links.
 */
import { describe, it, expect } from "vitest";
import { splitLinks } from "./linkify";

const links = (text: string) =>
  splitLinks(text)
    .filter((p) => p.kind === "link")
    .map((p) => p.text);

describe("splitLinks", () => {
  it("本文に http と https のアドレスがあるとき、それぞれがリンクになること", () => {
    expect(links("a http://example.com b https://example.com/x?y=1#z c")).toEqual([
      "http://example.com",
      "https://example.com/x?y=1#z",
    ]);
  });

  it("アドレスの前後に日本語があるとき、日本語はリンクに含まれないこと", () => {
    expect(splitLinks("見て https://example.com/a を開いて")).toEqual([
      { kind: "text", text: "見て " },
      { kind: "link", text: "https://example.com/a", url: "https://example.com/a" },
      { kind: "text", text: " を開いて" },
    ]);
  });

  it("アドレスの直後に句読点があるとき、句読点はリンクに含まれないこと", () => {
    expect(links("see https://example.com/a.")).toEqual(["https://example.com/a"]);
    expect(links("(https://example.com/a)")).toEqual(["https://example.com/a"]);
    expect(links("https://example.com/a、次へ")).toEqual(["https://example.com/a"]);
    expect(links("https://example.com/a?!")).toEqual(["https://example.com/a"]);
  });

  it("アドレスの中に対の括弧があるとき、括弧はリンクに含まれたままであること", () => {
    expect(links("https://example.com/wiki/Foo_(bar)")).toEqual([
      "https://example.com/wiki/Foo_(bar)",
    ]);
  });

  it("http と https 以外のスキームのとき、リンクにならないこと", () => {
    expect(links("file:///etc/passwd javascript:alert(1) ftp://example.com mailto:a@b.c")).toEqual([]);
  });

  it("スキームだけでホストが無いとき、リンクにならないこと", () => {
    expect(links(`${"https"}:// や ${"http"}:///path`)).toEqual([]);
  });

  it("リンクが無いとき、本文全体が 1 つのテキストになること", () => {
    expect(splitLinks("こんにちは")).toEqual([{ kind: "text", text: "こんにちは" }]);
    expect(splitLinks("")).toEqual([]);
  });

  it("改行をはさんでアドレスが並ぶとき、それぞれが別のリンクになること", () => {
    expect(links(["https://a.example.com", "https://b.example.com"].join("\n"))).toEqual([
      "https://a.example.com",
      "https://b.example.com",
    ]);
  });
});
