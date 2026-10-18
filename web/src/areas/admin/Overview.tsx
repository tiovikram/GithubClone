import { Link } from "react-router-dom";
import type { AdminCtx } from "../types";
import { Async } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi } from "./api";
import { PageHead, Pill, Stat } from "./ui";

const groups: { key: string; title: string; link?: string; fields: [string, string][] }[] = [
  { key: "users", title: "Users", link: "users", fields: [["total_users", "Total"], ["admin_users", "Site admins"], ["suspended_users", "Suspended"]] },
  { key: "orgs", title: "Organizations", link: "organizations", fields: [["total_orgs", "Total"], ["total_teams", "Teams"], ["total_team_members", "Team members"]] },
  { key: "repos", title: "Repositories", link: "repositories", fields: [["total_repos", "Total"], ["root_repos", "Root"], ["fork_repos", "Forks"], ["org_repos", "Org-owned"], ["total_pushes", "Pushes"]] },
  { key: "issues", title: "Issues", fields: [["total_issues", "Total"], ["open_issues", "Open"], ["closed_issues", "Closed"]] },
  { key: "pulls", title: "Pull requests", fields: [["total_pulls", "Total"], ["merged_pulls", "Merged"], ["mergeable_pulls", "Mergeable"], ["unmergeable_pulls", "Unmergeable"]] },
  { key: "milestones", title: "Milestones", fields: [["total_milestones", "Total"], ["open_milestones", "Open"], ["closed_milestones", "Closed"]] },
  { key: "comments", title: "Comments", fields: [["total_issue_comments", "Issue"], ["total_pull_request_comments", "Pull request"], ["total_commit_comments", "Commit"], ["total_gist_comments", "Gist"]] },
  { key: "gists", title: "Gists", fields: [["total_gists", "Total"], ["public_gists", "Public"], ["private_gists", "Secret"]] },
  { key: "hooks", title: "Webhooks", link: "hooks", fields: [["total_hooks", "Total"], ["active_hooks", "Active"], ["inactive_hooks", "Inactive"]] },
  { key: "pages", title: "Pages", fields: [["total_pages", "Sites"]] },
];

export function OverviewSection({ ctx }: { ctx: AdminCtx }) {
  const stats = useAsync(() => adminApi.stats(), []);
  const lic = useAsync(() => adminApi.license(), []);
  const maint = useAsync(() => adminApi.maintenanceState(), []);
  return (
    <div data-testid="sa-overview">
      <PageHead title="Overview" desc={<>Instance statistics for the <strong>{ctx.enterprise}</strong> enterprise.</>} />
      <div className="sa-cards">
        <div className="box box-body sa-card">
          <h3 className="h5">Maintenance mode</h3>
          {maint.data ? <Pill>{maint.data.status}</Pill> : <span className="muted">…</span>}
          <Link className="sa-card-link small" to="/site-admin/maintenance">Manage</Link>
        </div>
        <div className="box box-body sa-card">
          <h3 className="h5">License</h3>
          {lic.data ? <span>{lic.data.seats_used} of {String(lic.data.seats)} seats used</span> : <span className="muted">…</span>}
          <Link className="sa-card-link small" to="/site-admin/license">Details</Link>
        </div>
      </div>
      <Async state={stats}>
        {(s) => (
          <div className="sa-stat-groups">
            {groups.map((g) => (
              <section key={g.key} className="box" data-testid={`sa-stats-${g.key}`}>
                <div className="box-header">
                  <h3 className="h5">{g.title}</h3>
                  {g.link && <Link className="small sa-card-link" to={`/site-admin/${g.link}`}>View</Link>}
                </div>
                <div className="sa-stat-row">
                  {g.fields.map(([k, label]) => <Stat key={k} label={label} value={s[g.key]?.[k]} />)}
                </div>
              </section>
            ))}
          </div>
        )}
      </Async>
    </div>
  );
}
