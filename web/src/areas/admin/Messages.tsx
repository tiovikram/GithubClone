// Announcement banner and maintenance mode.
import { useEffect, useState, type FormEvent } from "react";
import { Async, Flash, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { fullDate } from "../../util";
import { adminApi, type Announcement, type ManageMaintenance, type MaintenanceState } from "./api";
import { fromLocalInput, PageHead, Pill, toLocalInput, useAction } from "./ui";

export function AnnouncementSection() {
  const state = useAsync(() => adminApi.announcement(), []);
  return (
    <div data-testid="sa-announcement">
      <PageHead title="Announcement" desc="A banner shown at the top of every page for every user (Markdown supported)." />
      <Async state={state}>{(a) => <AnnouncementForm a={a} reload={state.reload} />}</Async>
    </div>
  );
}

function AnnouncementForm({ a, reload }: { a: Announcement; reload: () => void }) {
  const [text, setText] = useState(a.announcement || "");
  const [expires, setExpires] = useState(toLocalInput(a.expires_at));
  const [dismissible, setDismissible] = useState(a.user_dismissible);
  const act = useAction();
  useEffect(() => {
    setText(a.announcement || "");
    setExpires(toLocalInput(a.expires_at));
    setDismissible(a.user_dismissible);
  }, [a]);
  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (await act.run(() => adminApi.setAnnouncement({ announcement: text, expires_at: fromLocalInput(expires), user_dismissible: dismissible }), "Announcement saved.")) reload();
  };
  return (
    <form className="box box-body" onSubmit={save} aria-label="Announcement">
      {act.status}
      <p className="small">
        Current: {a.announcement ? <><Pill kind="enabled">active</Pill>{a.expires_at ? <> · expires {fullDate(a.expires_at)}</> : " · no expiry"}{a.user_dismissible ? " · dismissible" : ""}</> : <Pill>off</Pill>}
      </p>
      <div className="form-group">
        <label htmlFor="sa-ann-text">Announcement</label>
        <textarea id="sa-ann-text" className="input textarea" rows={4} value={text} onChange={(e) => setText(e.target.value)}
          placeholder="Scheduled downtime on Saturday 10:00 UTC. **Save your work.**" />
      </div>
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="sa-ann-expires">Expires <span className="muted small">(optional, your local time)</span></label>
          <input id="sa-ann-expires" type="datetime-local" className="input input-auto" value={expires} onChange={(e) => setExpires(e.target.value)} />
        </div>
      </div>
      <label className="check-row"><input id="sa-ann-dismiss" type="checkbox" checked={dismissible} onChange={(e) => setDismissible(e.target.checked)} /> Allow users to dismiss the announcement</label>
      <div className="form-actions">
        <button type="button" className="btn btn-danger-outline" disabled={act.busy || !a.announcement} data-testid="sa-ann-clear"
          onClick={async () => { if (await act.run(() => adminApi.removeAnnouncement(), "Announcement removed.")) reload(); }}>Remove announcement</button>
        <button type="submit" className="btn btn-primary" disabled={act.busy || !text.trim()} data-testid="sa-ann-save">Save announcement</button>
      </div>
    </form>
  );
}

export function MaintenanceSection() {
  const manage = useAsync(() => adminApi.maintenance(), []);
  const state = useAsync(() => adminApi.maintenanceState(), []);
  const reload = () => { manage.reload(); state.reload(); };
  return (
    <div data-testid="sa-maintenance">
      <PageHead title="Maintenance mode"
        desc="While maintenance mode is on, everyone except site administrators (and the IP exception list) sees a maintenance page, and git and API requests get 503." />
      <Async state={manage}>
        {(nodes) => state.data ? <MaintenanceForm nodes={nodes} st={state.data} reload={reload} /> : <></>}
      </Async>
    </div>
  );
}

function MaintenanceForm({ nodes, st, reload }: { nodes: ManageMaintenance[]; st: MaintenanceState; reload: () => void }) {
  const first = nodes[0];
  const [message, setMessage] = useState(first?.maintenance_mode_message || "");
  const [when, setWhen] = useState(toLocalInput(st.scheduled_time));
  const [ips, setIps] = useState((first?.ip_exception_list || []).join("\n"));
  const act = useAction();
  const list = () => ips.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
  const enable = async (scheduled: boolean) => {
    const at = scheduled ? fromLocalInput(when) : null;
    if (scheduled && !at) {
      act.setError(new Error("Choose a date and time to schedule maintenance."));
      return;
    }
    if (await act.run(() => adminApi.setMaintenance({ enabled: true, when: at || "now", maintenance_mode_message: message, ip_exception_list: list() }),
      scheduled ? "Maintenance scheduled." : "Maintenance mode is on.")) reload();
  };
  return (
    <form className="box box-body" aria-label="Maintenance mode" onSubmit={(e) => { e.preventDefault(); void enable(false); }}>
      {act.status}
      <p data-testid="sa-maint-status">
        Status: <Pill>{st.status}</Pill>
        {st.status === "scheduled" && st.scheduled_time && <> — starts {fullDate(st.scheduled_time)} (<RelTime iso={st.scheduled_time} />)</>}
      </p>
      {st.active && <Flash kind="warn">Maintenance mode is on. Users see the maintenance page; you can still use the site because you are a site administrator.</Flash>}
      <div className="form-group">
        <label htmlFor="sa-maint-msg">Maintenance page message</label>
        <textarea id="sa-maint-msg" className="input textarea" rows={3} value={message} onChange={(e) => setMessage(e.target.value)}
          placeholder="We are upgrading forge. We'll be back shortly." />
      </div>
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="sa-maint-when">Schedule for <span className="muted small">(your local time)</span></label>
          <input id="sa-maint-when" type="datetime-local" className="input input-auto" value={when} onChange={(e) => setWhen(e.target.value)} />
        </div>
      </div>
      <div className="form-group">
        <label htmlFor="sa-maint-ips">IP exception list <span className="muted small">(addresses or CIDR ranges, one per line)</span></label>
        <textarea id="sa-maint-ips" className="input textarea mono" rows={2} value={ips} onChange={(e) => setIps(e.target.value)} placeholder="10.0.0.0/8" />
      </div>
      <div className="form-actions">
        <button type="button" className="btn" disabled={act.busy || st.status === "off"} data-testid="sa-maint-off"
          onClick={async () => { if (await act.run(() => adminApi.setMaintenance({ enabled: false }), "Maintenance mode is off.")) reload(); }}>Disable maintenance mode</button>
        <button type="button" className="btn" disabled={act.busy || !when} data-testid="sa-maint-schedule" onClick={() => enable(true)}>Schedule</button>
        <button type="submit" className="btn btn-danger" disabled={act.busy} data-testid="sa-maint-on">Enable now</button>
      </div>
      {nodes.length > 0 && (
        <p className="muted small">Nodes: {nodes.map((n) => `${n.hostname} (${n.status})`).join(", ")}</p>
      )}
    </form>
  );
}
