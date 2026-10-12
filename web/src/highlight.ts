import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const langs = { bash, c, cpp, css, diff, dockerfile, go, ini, java, javascript, json, makefile, markdown, python, ruby, rust, sql, typescript, xml, yaml };
for (const [name, def] of Object.entries(langs)) hljs.registerLanguage(name, def);

const EXT: Record<string, string> = {
  sh: "bash", bash: "bash", zsh: "bash", c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cxx: "cpp",
  css: "css", scss: "css", diff: "diff", patch: "diff", go: "go", toml: "ini", ini: "ini", cfg: "ini",
  java: "java", kt: "java", js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript",
  json: "json", md: "markdown", markdown: "markdown", py: "python", rb: "ruby", rs: "rust", sql: "sql",
  ts: "typescript", tsx: "typescript", mts: "typescript", html: "xml", htm: "xml", xml: "xml", svg: "xml",
  yml: "yaml", yaml: "yaml",
};
const NAMES: Record<string, string> = { Dockerfile: "dockerfile", Makefile: "makefile", "Cargo.lock": "ini" };

export function languageFor(filename: string): string | undefined {
  const base = filename.split("/").pop() || filename;
  if (NAMES[base]) return NAMES[base];
  const ext = base.includes(".") ? base.split(".").pop()!.toLowerCase() : "";
  return EXT[ext];
}

const escapeHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * Highlight `code` and return one HTML string per line. Spans that cross line
 * boundaries are closed at end-of-line and reopened on the next line.
 */
export function highlightLines(code: string, filename: string): string[] {
  const lang = languageFor(filename);
  if (!lang || code.length > 500_000) return code.split("\n").map(escapeHtml);
  let html: string;
  try {
    html = hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
  } catch {
    return code.split("\n").map(escapeHtml);
  }
  const out: string[] = [];
  const stack: string[] = [];
  let cur = "";
  const re = /(<span[^>]*>)|(<\/span>)|(\n)|([^<\n]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    if (m[1]) {
      stack.push(m[1]);
      cur += m[1];
    } else if (m[2]) {
      stack.pop();
      cur += m[2];
    } else if (m[3]) {
      out.push(cur + "</span>".repeat(stack.length));
      cur = stack.join("");
    } else cur += m[4];
  }
  out.push(cur + "</span>".repeat(stack.length));
  return out;
}

export function highlightBlock(code: string, lang?: string): string {
  if (lang && hljs.getLanguage(lang)) {
    try {
      return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
    } catch {
      /* fall through */
    }
  }
  return escapeHtml(code);
}
