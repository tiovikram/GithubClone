import { useMemo, useState, type ReactNode } from "react";
import type { CommitFile } from "../api";
import { ChevronDownIcon } from "./icons";
import { MarkdownEditor } from "./Conversation";

interface DiffLine {
  kind: "hunk" | "add" | "del" | "ctx" | "meta";
  text: string;
  oldNo?: number;
  newNo?: number;
}

export function parsePatch(patch: string): DiffLine[] {
  const out: DiffLine[] = [];
  let o = 0;
  let n = 0;
  const lines = patch.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    const h = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (h) {
      o = Number(h[1]);
      n = Number(h[2]);
      out.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) out.push({ kind: "add", text: line.slice(1), newNo: n++ });
    else if (line.startsWith("-")) out.push({ kind: "del", text: line.slice(1), oldNo: o++ });
    else if (line.startsWith("\\")) out.push({ kind: "meta", text: line });
    else out.push({ kind: "ctx", text: line.slice(1), oldNo: o++, newNo: n++ });
  }
  return out;
}

function DiffStat({ add, del }: { add: number; del: number }) {
  const total = add + del;
  const blocks = 5;
  const g = total ? Math.round((add / total) * blocks) : 0;
  const r = total ? Math.min(blocks - g, Math.round((del / total) * blocks)) : 0;
  return (
    <span className="diffstat" aria-label={`${add} additions, ${del} deletions`}>
      <span className="c-add">+{add}</span> <span className="c-del">−{del}</span>{" "}
      <span className="diffstat-blocks" aria-hidden="true">
        {Array.from({ length: blocks }, (_, i) => (
          <i key={i} className={i < g ? "b-add" : i < g + r ? "b-del" : "b-neutral"} />
        ))}
      </span>
    </span>
  );
}

/** A new line comment as drafted on the Files tab. */
export interface LineCommentDraft {
  path: string;
  body: string;
  line: number;
  side: "LEFT" | "RIGHT";
  start_line?: number;
  start_side?: "LEFT" | "RIGHT";
}

/** Review support for the diff (pull request Files tab). */
export interface DiffReview {
  /** Existing threads, rendered below the line they belong to (`line` on `side`). */
  threadsAt: (path: string, side: "LEFT" | "RIGHT", line: number) => ReactNode[];
  /** Pending (not yet submitted) review comments. */
  pendingAt?: (path: string, side: "LEFT" | "RIGHT", line: number) => ReactNode[];
  canComment: boolean;
  /** Whether a review is in progress (changes the primary button to "Add review comment"). */
  reviewing: boolean;
  onComment: (c: LineCommentDraft, mode: "single" | "review") => Promise<void>;
  repoPath: string;
}

type Side = "LEFT" | "RIGHT";
const sideOf = (l: DiffLine): Side => (l.kind === "del" ? "LEFT" : "RIGHT");
const numOf = (l: DiffLine): number => (l.kind === "del" ? l.oldNo! : l.newNo!);

function LineCommentForm({ path, start, end, lines, review, onDone }: {
  path: string;
  start: DiffLine;
  end: DiffLine;
  lines: string[];
  review: DiffReview;
  onDone: () => void;
}) {
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const multi = start !== end;
  const submit = async (mode: "single" | "review") => {
    setBusy(true);
    setErr("");
    try {
      await review.onComment({
        path, body, line: numOf(end), side: sideOf(end),
        ...(multi ? { start_line: numOf(start), start_side: sideOf(start) } : {}),
      }, mode);
      onDone();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  return (
    <form className="pl-line-form" aria-label="Add line comment" onSubmit={(e) => { e.preventDefault(); submit(review.reviewing ? "review" : "single"); }}>
      <div className="muted small pl-line-form-head">
        {multi ? `Commenting on lines ${sideOf(start) === "LEFT" ? "L" : "R"}${numOf(start)} to ${sideOf(end) === "LEFT" ? "L" : "R"}${numOf(end)}` :
          `Commenting on line ${sideOf(end) === "LEFT" ? "L" : "R"}${numOf(end)}`}
        <span className="spacer" />
        {sideOf(end) === "RIGHT" && (
          <button type="button" className="btn btn-sm" data-testid="insert-suggestion" title="Insert a suggestion"
            onClick={() => setBody((b) => `${b}${b && !b.endsWith("\n") ? "\n" : ""}\`\`\`suggestion\n${lines.join("\n")}\n\`\`\`\n`)}>
            ± Suggest
          </button>
        )}
      </div>
      <MarkdownEditor value={body} onChange={setBody} label="Line comment" placeholder="Leave a comment" rows={4} repoPath={review.repoPath} />
      {err && <p className="c-danger small" role="alert">{err}</p>}
      <div className="form-actions">
        <button type="button" className="btn btn-sm" onClick={onDone}>Cancel</button>
        {review.reviewing ? (
          <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !body.trim()}>Add review comment</button>
        ) : (
          <>
            <button type="button" className="btn btn-sm" disabled={busy || !body.trim()} onClick={() => submit("single")}>Add single comment</button>
            <button type="button" className="btn btn-sm btn-primary" disabled={busy || !body.trim()} onClick={() => submit("review")}>Start a review</button>
          </>
        )}
      </div>
    </form>
  );
}

function FileDiff({ file, review }: { file: CommitFile; review?: DiffReview }) {
  const [open, setOpen] = useState(true);
  const lines = useMemo(() => (file.patch ? parsePatch(file.patch) : []), [file.patch]);
  const [sel, setSel] = useState<{ start: number; end: number } | null>(null);
  const [formAt, setFormAt] = useState<number | null>(null);
  const id = "diff-" + file.filename.replace(/[^a-zA-Z0-9]/g, "-");
  const commentable = (l: DiffLine) => l.kind === "add" || l.kind === "del" || l.kind === "ctx";
  const pick = (i: number, extend: boolean) => {
    if (extend && sel && formAt !== null) {
      // Extend a selection within the same hunk and side kind.
      const a = Math.min(sel.start, i);
      const b = Math.max(sel.start, i);
      setSel({ start: a, end: b });
      setFormAt(b);
    } else {
      setSel({ start: i, end: i });
      setFormAt(i);
    }
  };
  const inSel = (i: number) => !!sel && formAt !== null && i >= sel.start && i <= sel.end;
  return (
    <div className="file-diff box" id={id} data-path={file.filename}>
      <div className="file-diff-header">
        <button type="button" className="btn-link btn-icon" aria-expanded={open}
          aria-label={open ? "Collapse file" : "Expand file"} onClick={() => setOpen(!open)}>
          <ChevronDownIcon style={{ transform: open ? undefined : "rotate(-90deg)" }} />
        </button>
        <DiffStat add={file.additions} del={file.deletions} />
        <span className="mono file-diff-name">
          {file.previous_filename && file.status === "renamed" ? `${file.previous_filename} → ` : ""}
          {file.filename}
        </span>
        {file.status !== "modified" && <span className={`badge badge-${file.status}`}>{file.status}</span>}
      </div>
      {open && (
        <div className="diff-body">
          {lines.length === 0 ? (
            <div className="muted diff-empty">
              {file.status === "renamed" ? "File renamed without changes." : "Binary file or diff too large to display."}
            </div>
          ) : (
            <table className="diff-table">
              <tbody>
                {lines.map((l, i) => {
                  if (l.kind === "hunk") {
                    return (
                      <tr key={i} className="d-hunk">
                        <td className="ln" colSpan={2} />
                        <td className="code">{l.text}</td>
                      </tr>
                    );
                  }
                  const canC = !!review?.canComment && commentable(l);
                  const after: ReactNode[] = [];
                  if (review && commentable(l)) {
                    // Threads on the new side attach to add/ctx lines; on the old side to del/ctx lines.
                    if (l.newNo !== undefined) after.push(...review.threadsAt(file.filename, "RIGHT", l.newNo), ...(review.pendingAt?.(file.filename, "RIGHT", l.newNo) || []));
                    if (l.oldNo !== undefined && l.kind === "del") after.push(...review.threadsAt(file.filename, "LEFT", l.oldNo), ...(review.pendingAt?.(file.filename, "LEFT", l.oldNo) || []));
                  }
                  return (
                    <FragmentRows key={i}>
                      <tr className={`d-${l.kind}${inSel(i) ? " d-selected" : ""}`} data-old={l.oldNo ?? ""} data-new={l.newNo ?? ""}>
                        <td className="ln" data-ln={l.oldNo ?? ""} />
                        <td className="ln" data-ln={l.newNo ?? ""}>
                          {canC && (
                            <button type="button" className="pl-add-line" aria-label={`Comment on line ${l.kind === "del" ? "L" + l.oldNo : "R" + l.newNo}`}
                              data-testid="add-line-comment" onClick={(e) => pick(i, e.shiftKey)}>+</button>
                          )}
                        </td>
                        <td className="code">
                          <span className="d-marker" aria-hidden="true">
                            {l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "}
                          </span>
                          {l.text}
                        </td>
                      </tr>
                      {after.length > 0 && (
                        <tr className="d-threads"><td colSpan={3}>{after}</td></tr>
                      )}
                      {review && formAt === i && sel && (
                        <tr className="d-threads">
                          <td colSpan={3}>
                            <LineCommentForm path={file.filename} start={lines[sel.start]} end={lines[sel.end]}
                              lines={lines.slice(sel.start, sel.end + 1).filter((x) => x.kind !== "del").map((x) => x.text)}
                              review={review} onDone={() => { setFormAt(null); setSel(null); }} />
                          </td>
                        </tr>
                      )}
                    </FragmentRows>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}

function FragmentRows({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

export function DiffView({ files, review, toolbar }: { files: CommitFile[]; review?: DiffReview; toolbar?: ReactNode }) {
  const add = files.reduce((a, f) => a + f.additions, 0);
  const del = files.reduce((a, f) => a + f.deletions, 0);
  if (!files.length) return <p className="muted">No changed files.</p>;
  return (
    <div className="diff-view">
      <div className="diff-summary">
        <span>
          Showing <strong>{files.length} changed file{files.length === 1 ? "" : "s"}</strong> with{" "}
          <strong className="c-add">{add} addition{add === 1 ? "" : "s"}</strong> and{" "}
          <strong className="c-del">{del} deletion{del === 1 ? "" : "s"}</strong>.
        </span>
        {toolbar}
      </div>
      {files.map((f) => (
        <FileDiff key={f.filename} file={f} review={review} />
      ))}
    </div>
  );
}
