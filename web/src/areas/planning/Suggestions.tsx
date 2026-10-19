// Comment bodies with GitHub "suggested change" blocks (```suggestion fences) and applying a
// suggestion as a commit on the pull request's head branch.
import { useState } from "react";
import { decodeBase64, errorMessage } from "../../api";
import { Markdown } from "../../markdown";
import { b64encode, papi } from "./api";

const FENCE = /```suggestion[^\n]*\n([\s\S]*?)(?:\n)?```/g;

export function hasSuggestion(body: string | null | undefined): boolean {
  return !!body && /```suggestion/.test(body);
}

/** The replacement text of the first suggestion block in `body`. */
export function firstSuggestion(body: string): string | null {
  FENCE.lastIndex = 0;
  const m = FENCE.exec(body);
  return m ? m[1] : null;
}

export interface SuggestionContext {
  /** Lines the suggestion replaces (from the diff), shown as removed. */
  original: string[];
  canApply: boolean;
  onApply?: (replacement: string) => Promise<void>;
}

export function CommentBody({ body, repoPath, suggestion }: { body: string | null; repoPath: string; suggestion?: SuggestionContext }) {
  if (!body || !hasSuggestion(body)) return <Markdown source={body} comment repoPath={repoPath} />;
  const parts: { text?: string; sug?: string }[] = [];
  let last = 0;
  FENCE.lastIndex = 0;
  for (let m = FENCE.exec(body); m; m = FENCE.exec(body)) {
    if (m.index > last) parts.push({ text: body.slice(last, m.index) });
    parts.push({ sug: m[1] });
    last = m.index + m[0].length;
  }
  if (last < body.length) parts.push({ text: body.slice(last) });
  return (
    <>
      {parts.map((p, i) => p.sug !== undefined ? (
        <SuggestionBlock key={i} replacement={p.sug} ctx={suggestion} />
      ) : p.text!.trim() ? <Markdown key={i} source={p.text!} comment repoPath={repoPath} /> : null)}
    </>
  );
}

function SuggestionBlock({ replacement, ctx }: { replacement: string; ctx?: SuggestionContext }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [done, setDone] = useState(false);
  const added = replacement === "" ? [] : replacement.split("\n");
  return (
    <div className="pl-suggestion box" data-testid="suggestion">
      <div className="pl-suggestion-head small">
        <strong>Suggested change</strong>
        <span className="spacer" />
        {ctx?.canApply && ctx.onApply && (
          done ? <span className="c-open small">Suggestion applied</span> : (
            <button type="button" className="btn btn-sm" disabled={busy} data-testid="apply-suggestion" onClick={async () => {
              setBusy(true);
              setErr("");
              try {
                await ctx.onApply!(replacement);
                setDone(true);
              } catch (e) {
                setErr(errorMessage(e));
              } finally {
                setBusy(false);
              }
            }}>{busy ? "Committing…" : "Commit suggestion"}</button>
          )
        )}
      </div>
      <table className="diff-table pl-suggestion-diff">
        <tbody>
          {(ctx?.original || []).map((l, i) => (
            <tr key={`o${i}`} className="d-del"><td className="code"><span className="d-marker" aria-hidden="true">-</span>{l}</td></tr>
          ))}
          {added.map((l, i) => (
            <tr key={`a${i}`} className="d-add"><td className="code"><span className="d-marker" aria-hidden="true">+</span>{l}</td></tr>
          ))}
        </tbody>
      </table>
      {err && <p className="c-danger small pl-suggestion-err" role="alert">{err}</p>}
    </div>
  );
}

/**
 * Commit a suggestion: replace lines [start, end] (1-based, inclusive) of `path` on `branch`
 * of owner/name with `replacement`.
 */
export async function applySuggestion(owner: string, name: string, branch: string, path: string, start: number, end: number, replacement: string) {
  const f = await papi.file(owner, name, path, branch);
  const text = decodeBase64(f.content);
  const trailingNl = text.endsWith("\n");
  const lines = (trailingNl ? text.slice(0, -1) : text).split("\n");
  const repl = replacement === "" ? [] : replacement.split("\n");
  lines.splice(start - 1, end - start + 1, ...repl);
  const out = lines.join("\n") + (trailingNl ? "\n" : "");
  await papi.putFile(owner, name, path, { message: "Apply suggestions from code review", content: b64encode(out), sha: f.sha, branch });
}

/** Lines [start, end] of the new side of a diff hunk (the comment's `diff_hunk` ends at `end`). */
export function hunkLines(diffHunk: string, start: number, end: number, side: "LEFT" | "RIGHT"): string[] {
  const lines = diffHunk.split("\n");
  let o = 0;
  let n = 0;
  const out: string[] = [];
  for (const line of lines) {
    const h = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (h) {
      o = Number(h[1]);
      n = Number(h[2]);
      continue;
    }
    const k = line[0];
    const text = line.slice(1);
    if (k === "+") {
      if (side === "RIGHT" && n >= start && n <= end) out.push(text);
      n++;
    } else if (k === "-") {
      if (side === "LEFT" && o >= start && o <= end) out.push(text);
      o++;
    } else if (k === "\\") {
      /* no newline marker */
    } else {
      if ((side === "RIGHT" ? n : o) >= start && (side === "RIGHT" ? n : o) <= end) out.push(text);
      o++;
      n++;
    }
  }
  return out;
}
