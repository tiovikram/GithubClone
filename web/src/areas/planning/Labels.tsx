// `/{o}/{r}/labels`: list, create, edit and delete labels (name, description, color).
import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { errorMessage, type Label } from "../../api";
import { Async, Blankslate, LabelPill } from "../../components/common";
import { TagIcon } from "../../components/icons";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { papi } from "./api";
import { confirmAction, randomColor } from "./ui";

function LabelForm({ initial, onSave, onCancel, submitText }: {
  initial: { name: string; description: string; color: string };
  onSave: (v: { name: string; description: string; color: string }) => Promise<void>;
  onCancel: () => void;
  submitText: string;
}) {
  const [name, setName] = useState(initial.name);
  const [description, setDescription] = useState(initial.description);
  const [color, setColor] = useState(initial.color);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const valid = /^[0-9a-fA-F]{6}$/.test(color);
  return (
    <form className="pl-label-form" aria-label={submitText} onSubmit={async (e) => {
      e.preventDefault();
      setBusy(true);
      setErr("");
      try {
        await onSave({ name: name.trim(), description: description.trim(), color: color.toLowerCase() });
      } catch (ex) {
        setErr(errorMessage(ex));
        setBusy(false);
      }
    }}>
      <div className="pl-label-preview"><LabelPill label={{ name: name || "Label preview", color: valid ? color : "ededed", description: null }} /></div>
      <div className="pl-label-fields">
        <div className="form-group">
          <label htmlFor={`ln-${initial.name}`}>Label name</label>
          <input id={`ln-${initial.name}`} className="input" required value={name} onChange={(e) => setName(e.target.value)} placeholder="Label name" maxLength={50} />
        </div>
        <div className="form-group">
          <label htmlFor={`ld-${initial.name}`}>Description</label>
          <input id={`ld-${initial.name}`} className="input" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="Description (optional)" maxLength={100} />
        </div>
        <div className="form-group">
          <label htmlFor={`lc-${initial.name}`}>Color</label>
          <div className="pl-color-row">
            <button type="button" className="btn btn-sm" aria-label="Get a new color" title="Get a new color" onClick={() => setColor(randomColor())}
              style={{ background: valid ? `#${color}` : undefined }}>↻</button>
            <input id={`lc-${initial.name}`} className="input input-sm mono" value={`#${color}`} aria-invalid={!valid}
              onChange={(e) => setColor(e.target.value.replace(/^#/, "").slice(0, 6))} />
            <input type="color" aria-label="Pick a color" value={valid ? `#${color}` : "#ededed"} onChange={(e) => setColor(e.target.value.slice(1))} />
          </div>
        </div>
        <div className="form-actions pl-label-actions">
          <button type="button" className="btn" onClick={onCancel}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !name.trim() || !valid}>{busy ? "Saving…" : submitText}</button>
        </div>
      </div>
      {err && <p className="c-danger small" role="alert">{err}</p>}
    </form>
  );
}

export function LabelsPage() {
  const { repo, owner, name, base, canPush } = useRepo();
  useTitle(`Labels · ${repo.full_name}`);
  const st = useAsync(() => papi.labels(owner, name), [owner, name]);
  const [sp, setSp] = useSearchParams();
  const q = sp.get("q") || "";
  const sort = sp.get("sort") || "name-asc";
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [err, setErr] = useState("");
  return (
    <div>
      <div className="toolbar">
        <Link to={`${base}/labels`} className="btn btn-sm selected">Labels</Link>
        <Link to={`${base}/milestones`} className="btn btn-sm">Milestones</Link>
        <input className="input input-sm pl-grow-input" placeholder="Search all labels" aria-label="Search all labels" value={q}
          onChange={(e) => { const n = new URLSearchParams(sp); if (e.target.value) n.set("q", e.target.value); else n.delete("q"); setSp(n, { replace: true }); }} />
        <span className="spacer" />
        {canPush && <button type="button" className="btn btn-primary" onClick={() => setCreating(true)} data-testid="new-label">New label</button>}
      </div>
      {creating && (
        <div className="box box-body pl-new-label">
          <LabelForm initial={{ name: "", description: "", color: randomColor() }} submitText="Create label" onCancel={() => setCreating(false)}
            onSave={async (v) => {
              const l = await papi.createLabel(owner, name, v);
              st.setData([...(st.data || []), l]);
              setCreating(false);
            }} />
        </div>
      )}
      {err && <p className="c-danger" role="alert">{err}</p>}
      <Async state={st}>
        {(labels) => {
          const shown = labels
            .filter((l) => !q || l.name.toLowerCase().includes(q.toLowerCase()) || (l.description || "").toLowerCase().includes(q.toLowerCase()))
            .sort((a, b) => (sort === "name-desc" ? b.name.localeCompare(a.name) : a.name.localeCompare(b.name)));
          return (
            <div className="box">
              <div className="box-header pl-list-head">
                <strong>{shown.length} label{shown.length === 1 ? "" : "s"}</strong>
                <span className="spacer" />
                <label className="small muted" htmlFor="label-sort">Sort</label>{" "}
                <select id="label-sort" className="input input-sm input-auto" value={sort}
                  onChange={(e) => { const n = new URLSearchParams(sp); n.set("sort", e.target.value); setSp(n, { replace: true }); }}>
                  <option value="name-asc">Alphabetically</option>
                  <option value="name-desc">Reverse alphabetically</option>
                </select>
              </div>
              {shown.length === 0 ? <Blankslate title="No labels" icon={<TagIcon size={24} />}>There aren't any labels{q ? " matching your search" : " yet"}.</Blankslate> : (
                <ul className="list">
                  {shown.map((l: Label) => (
                    <li key={l.id} className="list-row pl-label-row" data-testid="label-row" data-label={l.name}>
                      {editing === l.name ? (
                        <LabelForm initial={{ name: l.name, description: l.description || "", color: l.color }} submitText="Save changes"
                          onCancel={() => setEditing(null)}
                          onSave={async (v) => {
                            const u = await papi.updateLabel(owner, name, l.name, { new_name: v.name, description: v.description, color: v.color });
                            st.setData((st.data || []).map((x) => (x.id === l.id ? u : x)));
                            setEditing(null);
                          }} />
                      ) : (
                        <>
                          <div className="pl-label-cell"><Link to={`${base}/issues?labels=${encodeURIComponent(l.name)}`}><LabelPill label={l} /></Link></div>
                          <div className="pl-label-desc muted small">{l.description}</div>
                          <Link className="muted-link small pl-label-count" to={`${base}/issues?labels=${encodeURIComponent(l.name)}`}>Open issues</Link>
                          {canPush && (
                            <div className="pl-label-btns">
                              <button type="button" className="btn-link small" onClick={() => setEditing(l.name)}>Edit</button>
                              <button type="button" className="btn-link small c-danger" onClick={async () => {
                                if (!confirmAction("Are you sure? Deleting a label will remove it from all issues and pull requests.")) return;
                                setErr("");
                                try {
                                  await papi.deleteLabel(owner, name, l.name);
                                  st.setData((st.data || []).filter((x) => x.id !== l.id));
                                } catch (e) { setErr(errorMessage(e)); }
                              }}>Delete</button>
                            </div>
                          )}
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          );
        }}
      </Async>
    </div>
  );
}
