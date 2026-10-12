import DOMPurify from "dompurify";
import { Marked } from "marked";
import { useMemo } from "react";
import { highlightBlock } from "./highlight";

const codeRenderer = {
  renderer: {
    code({ text, lang }: { text: string; lang?: string }) {
      const l = (lang || "").split(/\s/)[0];
      return `<pre><code class="hljs${l ? " language-" + l : ""}">${highlightBlock(text, l)}</code></pre>\n`;
    },
  },
};
const marked = new Marked({ gfm: true, breaks: false }, codeRenderer);
const markedBreaks = new Marked({ gfm: true, breaks: true }, codeRenderer);

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A" && node.getAttribute("href")?.match(/^https?:/)) {
    node.setAttribute("rel", "nofollow noopener noreferrer");
    node.setAttribute("target", "_blank");
  }
});

export interface MarkdownProps {
  source: string | null | undefined;
  /** Comments use GitHub's "breaks" flavor; READMEs do not. */
  comment?: boolean;
  /** Repo context for #123 references. */
  repoPath?: string;
}

export function renderMarkdown(src: string, comment = false, repoPath?: string): string {
  let text = src;
  if (comment && repoPath) {
    // Link bare #123 references (not inside code) to the issue page.
    text = text.replace(/(^|[\s(])#(\d+)\b/g, (_m, pre, n) => `${pre}[#${n}](/${repoPath}/issues/${n})`);
  }
  const html = (comment ? markedBreaks : marked).parse(text, { async: false }) as string;
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true } });
}

export function Markdown({ source, comment, repoPath }: MarkdownProps) {
  const html = useMemo(() => renderMarkdown(source || "", comment, repoPath), [source, comment, repoPath]);
  if (!source || !source.trim()) return <p className="muted"><em>No description provided.</em></p>;
  return <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />;
}
