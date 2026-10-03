import type { ReactNode } from "react";
import "./Terms.css";

export interface TermsTextProps {
  /** The text. Markdown for the terms of use, plain text for the license */
  text: string;
  format?: "markdown" | "plain";
  /** Leave out the first heading, for a container that already shows it as its title */
  skipTitle?: boolean;
}

type Block =
  | { kind: "h1" | "h2" | "p"; text: string }
  | { kind: "item"; text: string; depth: number }
  | { kind: "rule" };

/** The Markdown the terms use: two heading levels, nested numbered and bulleted lists, rules and paragraphs. */
export function parseTerms(text: string): Block[] {
  const blocks: Block[] = [];
  for (const line of text.split("\n")) {
    if (line.trim() === "") continue;
    if (/^-{3,}$/.test(line.trim())) {
      blocks.push({ kind: "rule" });
    } else if (line.startsWith("## ")) {
      blocks.push({ kind: "h2", text: line.slice(3) });
    } else if (line.startsWith("# ")) {
      blocks.push({ kind: "h1", text: line.slice(2) });
    } else if (/^\s*(\d+\.|-)\s/.test(line)) {
      const indent = line.length - line.trimStart().length;
      blocks.push({ kind: "item", text: line.trim(), depth: Math.floor(indent / 3) });
    } else {
      blocks.push({ kind: "p", text: line.trim() });
    }
  }
  return blocks;
}

/**
 * Bold text is shown bold. A link is shown as its label only: the terms are
 * read inside the app's own window, which must not navigate away from the app.
 */
function inline(text: string): ReactNode[] {
  return text
    .split(/(\*\*[^*]+\*\*|\[[^\]]+\]\([^)]+\))/)
    .filter((part) => part !== "")
    .map((part, index) => {
      if (part.startsWith("**")) return <strong key={index}>{part.slice(2, -2)}</strong>;
      const link = /^\[([^\]]+)\]\([^)]+\)$/.exec(part);
      return link ? <span key={index}>{link[1]}</span> : part;
    });
}

/** Terms of use or license text, scrolled by its container. */
export function TermsText({ text, format = "markdown", skipTitle = false }: TermsTextProps) {
  if (format === "plain") {
    return <pre className="terms-text terms-text--plain">{text}</pre>;
  }
  return (
    <div className="terms-text">
      {parseTerms(text).map((block, index) => {
        if (skipTitle && index === 0 && block.kind === "h1") return null;
        switch (block.kind) {
          case "h1":
            return (
              <h2 key={index} className="terms-text__h1">
                {inline(block.text)}
              </h2>
            );
          case "h2":
            return (
              <h3 key={index} className="terms-text__h2">
                {inline(block.text)}
              </h3>
            );
          case "item":
            return (
              <p
                key={index}
                className="terms-text__item"
                style={{ paddingLeft: `${block.depth * 20}px` }}
              >
                {inline(block.text)}
              </p>
            );
          case "rule":
            return <hr key={index} className="terms-text__rule" />;
          default:
            return (
              <p key={index} className="terms-text__p">
                {inline(block.text)}
              </p>
            );
        }
      })}
    </div>
  );
}
