import { useMemo, useState } from "react";
import type { CommitFile } from "../api";
import { ChevronDownIcon } from "./icons";

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

function FileDiff({ file }: { file: CommitFile }) {
  const [open, setOpen] = useState(true);
  const lines = useMemo(() => (file.patch ? parsePatch(file.patch) : []), [file.patch]);
  const id = "diff-" + file.filename.replace(/[^a-zA-Z0-9]/g, "-");
  return (
    <div className="file-diff box" id={id}>
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
                {lines.map((l, i) =>
                  l.kind === "hunk" ? (
                    <tr key={i} className="d-hunk">
                      <td className="ln" colSpan={2} />
                      <td className="code">{l.text}</td>
                    </tr>
                  ) : (
                    <tr key={i} className={`d-${l.kind}`}>
                      <td className="ln" data-ln={l.oldNo ?? ""} />
                      <td className="ln" data-ln={l.newNo ?? ""} />
                      <td className="code">
                        <span className="d-marker" aria-hidden="true">
                          {l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "}
                        </span>
                        {l.text}
                      </td>
                    </tr>
                  ),
                )}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}

export function DiffView({ files }: { files: CommitFile[] }) {
  const add = files.reduce((a, f) => a + f.additions, 0);
  const del = files.reduce((a, f) => a + f.deletions, 0);
  if (!files.length) return <p className="muted">No changed files.</p>;
  return (
    <div className="diff-view">
      <div className="diff-summary">
        Showing <strong>{files.length} changed file{files.length === 1 ? "" : "s"}</strong> with{" "}
        <strong className="c-add">{add} addition{add === 1 ? "" : "s"}</strong> and{" "}
        <strong className="c-del">{del} deletion{del === 1 ? "" : "s"}</strong>.
      </div>
      {files.map((f) => (
        <FileDiff key={f.filename} file={f} />
      ))}
    </div>
  );
}
