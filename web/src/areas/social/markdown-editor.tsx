// Textarea with Write / Preview tabs (GitHub's comment box).
import { useState } from "react";
import { Markdown } from "../../markdown";

export function MarkdownEditor({ id, value, onChange, label, placeholder, rows = 5 }: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  label: string;
  placeholder?: string;
  rows?: number;
}) {
  const [tab, setTab] = useState<"write" | "preview">("write");
  return (
    <div className="md-editor">
      <div className="md-editor-tabs" role="tablist" aria-label={label}>
        <button type="button" role="tab" aria-selected={tab === "write"} className={"md-tab" + (tab === "write" ? " selected" : "")} onClick={() => setTab("write")}>Write</button>
        <button type="button" role="tab" aria-selected={tab === "preview"} className={"md-tab" + (tab === "preview" ? " selected" : "")} onClick={() => setTab("preview")}
          data-testid="md-preview-tab">Preview</button>
      </div>
      {tab === "write" ? (
        <>
          <label htmlFor={id} className="sr-only">{label}</label>
          <textarea id={id} className="input" rows={rows} value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} />
        </>
      ) : (
        <div className="md-preview" data-testid="md-preview">{value.trim() ? <Markdown source={value} comment /> : <p className="muted">Nothing to preview</p>}</div>
      )}
      <p className="muted small md-hint">Markdown is supported.</p>
    </div>
  );
}
