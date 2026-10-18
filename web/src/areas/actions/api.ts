// REST helpers for the actions area: workflows, runs, jobs, logs, artifacts, caches, checks,
// statuses, deployments, environments, secrets/variables, runners, runner groups, permissions.
import { FORGE, request, requestPage, requestText, type Page, type Repo, type User } from "../../api";
import { sealSecret } from "./sealed";

const enc = encodeURIComponent;
const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;

// ---------- types ----------

export type Status = "queued" | "in_progress" | "completed" | "waiting" | "requested" | "pending" | "action_required" | string;
export type Conclusion = "success" | "failure" | "cancelled" | "skipped" | "neutral" | "timed_out" | "action_required" | "stale" | "startup_failure" | null | string;

export interface Workflow {
  id: number;
  name: string;
  path: string;
  state: "active" | "disabled_manually" | string;
  created_at: string;
  updated_at: string;
  html_url?: string;
}

export interface WorkflowRun {
  id: number;
  name: string;
  display_title: string;
  run_number: number;
  run_attempt: number;
  event: string;
  status: Status;
  conclusion: Conclusion;
  workflow_id: number;
  path: string;
  head_branch: string | null;
  head_sha: string;
  head_commit?: { id: string; message: string; timestamp: string; author?: { name: string; email: string } } | null;
  actor?: User | null;
  triggering_actor?: User | null;
  created_at: string;
  updated_at: string;
  run_started_at?: string;
  check_suite_id?: number;
  pull_requests?: { number: number; head: { ref: string }; base: { ref: string } }[];
  previous_attempt_url?: string | null;
}

export interface Step {
  name: string;
  number: number;
  status: Status;
  conclusion: Conclusion;
  started_at?: string | null;
  completed_at?: string | null;
}

export interface Job {
  id: number;
  run_id: number;
  run_attempt: number;
  name: string;
  status: Status;
  conclusion: Conclusion;
  started_at?: string | null;
  completed_at?: string | null;
  created_at?: string;
  steps?: Step[];
  labels: string[];
  runner_name?: string | null;
  runner_group_name?: string | null;
  workflow_name?: string;
  head_branch?: string;
  head_sha?: string;
  check_run_url?: string;
}

export interface Artifact {
  id: number;
  name: string;
  size_in_bytes: number;
  expired: boolean;
  created_at: string;
  expires_at: string;
  digest?: string;
  workflow_run?: { id: number; head_branch: string; head_sha: string };
}

export interface Cache {
  id: number;
  key: string;
  ref: string;
  version: string;
  size_in_bytes: number;
  created_at: string;
  last_accessed_at: string;
}

export interface CheckRun {
  id: number;
  name: string;
  head_sha: string;
  status: Status;
  conclusion: Conclusion;
  started_at: string | null;
  completed_at: string | null;
  details_url?: string | null;
  html_url?: string | null;
  external_id?: string | null;
  app?: { name: string; slug: string; owner?: unknown } | null;
  check_suite?: { id: number } | null;
  output: { title: string | null; summary: string | null; text: string | null; annotations_count: number };
}

export interface Annotation {
  path: string;
  start_line: number;
  end_line: number;
  annotation_level: "notice" | "warning" | "failure" | string;
  message: string;
  title: string | null;
}

export interface CommitStatus {
  id: number;
  state: "success" | "failure" | "error" | "pending" | string;
  context: string;
  description: string | null;
  target_url: string | null;
  created_at: string;
  updated_at: string;
  creator?: User | null;
}

export interface CombinedStatus {
  state: string;
  total_count: number;
  statuses: CommitStatus[];
  sha: string;
}

export interface Reviewer {
  type: "User" | "Team" | string;
  reviewer: { id: number; login?: string; name?: string; slug?: string; avatar_url?: string };
}

export interface ProtectionRule {
  id: number;
  type: "required_reviewers" | "wait_timer" | "branch_policy" | string;
  wait_timer?: number;
  prevent_self_review?: boolean;
  reviewers?: Reviewer[];
}

export interface Environment {
  id: number;
  name: string;
  html_url?: string;
  created_at: string;
  updated_at: string;
  protection_rules?: ProtectionRule[];
  deployment_branch_policy: { protected_branches: boolean; custom_branch_policies: boolean } | null;
  can_admins_bypass?: boolean;
}

export interface BranchPolicy {
  id: number;
  name: string;
  type?: "branch" | "tag" | string;
}

export interface PendingDeployment {
  environment: { id: number; name: string; html_url?: string };
  wait_timer: number;
  wait_timer_started_at: string | null;
  current_user_can_approve: boolean;
  reviewers: Reviewer[];
}

export interface Approval {
  state: "approved" | "rejected" | string;
  comment: string;
  user: User;
  environments: { id: number; name: string }[];
}

export interface Deployment {
  id: number;
  sha: string;
  ref: string;
  task: string;
  environment: string;
  description: string | null;
  creator: User | null;
  created_at: string;
  updated_at: string;
  statuses_url: string;
  production_environment?: boolean;
  transient_environment?: boolean;
}

export interface DeploymentStatus {
  id: number;
  state: "error" | "failure" | "inactive" | "in_progress" | "queued" | "pending" | "success" | string;
  description: string | null;
  environment?: string;
  environment_url?: string | null;
  log_url?: string | null;
  target_url?: string | null;
  creator?: User | null;
  created_at: string;
}

export interface Secret {
  name: string;
  created_at: string;
  updated_at: string;
  visibility?: "all" | "private" | "selected";
  selected_repositories_url?: string;
}

export interface Variable extends Secret {
  value: string;
}

export interface RunnerLabel {
  id?: number;
  name: string;
  type: "read-only" | "custom" | string;
}

export interface Runner {
  id: number;
  name: string;
  os: string;
  status: "online" | "offline" | string;
  busy: boolean;
  ephemeral?: boolean;
  labels: RunnerLabel[];
  runner_group_id?: number;
  version?: string;
}

export interface RunnerGroup {
  id: number;
  name: string;
  visibility: "all" | "selected" | "private" | string;
  default: boolean;
  inherited?: boolean;
  allows_public_repositories: boolean;
  restricted_to_workflows?: boolean;
  selected_workflows?: string[];
  runners_url?: string;
  selected_repositories_url?: string;
  selected_organizations_url?: string;
}

export interface ActionsPermissions {
  enabled?: boolean;
  enabled_repositories?: "all" | "none" | "selected";
  enabled_organizations?: "all" | "none" | "selected";
  allowed_actions?: "all" | "local_only" | "selected";
  sha_pinning_required?: boolean;
}

export interface SelectedActions {
  github_owned_allowed?: boolean;
  verified_allowed?: boolean;
  patterns_allowed?: string[];
}

export interface WorkflowPermissions {
  default_workflow_permissions: "read" | "write";
  can_approve_pull_request_reviews: boolean;
}

export interface OidcSub {
  use_default?: boolean;
  include_claim_keys?: string[];
}

export interface OrgSummary {
  id: number;
  login: string;
}

/** Where secrets/variables/runners live: a repository, an environment, an organization or the enterprise. */
export type Scope =
  | { kind: "repo"; owner: string; repo: string }
  | { kind: "env"; owner: string; repo: string; env: string }
  | { kind: "org"; org: string }
  | { kind: "enterprise"; slug: string };

export function scopeBase(s: Scope): string {
  switch (s.kind) {
    case "repo":
      return `${rp(s.owner, s.repo)}/actions`;
    case "env":
      return `${rp(s.owner, s.repo)}/environments/${enc(s.env)}`;
    case "org":
      return `/orgs/${enc(s.org)}/actions`;
    case "enterprise":
      return `/enterprises/${enc(s.slug)}/actions`;
  }
}

/** The enterprise slug (FORGE_ENTERPRISE_SLUG; `forge` unless configured otherwise). */
export const ENTERPRISE = "forge";

// ---------- endpoints ----------

export const actionsApi = {
  // workflows
  workflows: (o: string, r: string) => request<{ total_count: number; workflows: Workflow[] }>(`${rp(o, r)}/actions/workflows`, { query: { per_page: 100 } }),
  workflow: (o: string, r: string, id: string | number) => request<Workflow>(`${rp(o, r)}/actions/workflows/${enc(String(id))}`),
  enableWorkflow: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/workflows/${id}/enable`, { method: "PUT" }),
  disableWorkflow: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/workflows/${id}/disable`, { method: "PUT" }),
  dispatch: (o: string, r: string, id: number, ref: string, inputs: Record<string, string | boolean>) =>
    request<{ workflow_run_id?: number } | undefined>(`${rp(o, r)}/actions/workflows/${id}/dispatches`, { method: "POST", body: { ref, inputs, return_run_details: true } }),
  workflowFile: (o: string, r: string, path: string, ref?: string) =>
    requestText(`${rp(o, r)}/contents/${path.split("/").map(enc).join("/")}`, { query: { ref }, accept: "application/vnd.github.raw" }),

  // runs
  runs: (o: string, r: string, q: { workflow?: number; event?: string; status?: string; branch?: string; actor?: string; page?: number; head_sha?: string }) =>
    requestPage<{ total_count: number; workflow_runs: WorkflowRun[] }>(
      q.workflow ? `${rp(o, r)}/actions/workflows/${q.workflow}/runs` : `${rp(o, r)}/actions/runs`,
      { query: { event: q.event, status: q.status, branch: q.branch, actor: q.actor, head_sha: q.head_sha, per_page: 25, page: q.page || 1 } },
    ),
  run: (o: string, r: string, id: number) => request<WorkflowRun>(`${rp(o, r)}/actions/runs/${id}`),
  attempt: (o: string, r: string, id: number, n: number) => request<WorkflowRun>(`${rp(o, r)}/actions/runs/${id}/attempts/${n}`),
  jobs: (o: string, r: string, id: number, attempt?: number) =>
    request<{ total_count: number; jobs: Job[] }>(attempt ? `${rp(o, r)}/actions/runs/${id}/attempts/${attempt}/jobs` : `${rp(o, r)}/actions/runs/${id}/jobs`, { query: { per_page: 100, filter: attempt ? undefined : "latest" } }),
  job: (o: string, r: string, id: number) => request<Job>(`${rp(o, r)}/actions/jobs/${id}`),
  stepLogs: (o: string, r: string, id: number) =>
    request<{ job_id: number; status: string; steps: { number: number; log: string }[] }>(`${FORGE}${rp(o, r)}/actions/jobs/${id}/steps`),
  cancel: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/runs/${id}/cancel`, { method: "POST" }),
  forceCancel: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/runs/${id}/force-cancel`, { method: "POST" }),
  rerun: (o: string, r: string, id: number, debug = false) => request<void>(`${rp(o, r)}/actions/runs/${id}/rerun`, { method: "POST", body: { enable_debug_logging: debug } }),
  rerunFailed: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/runs/${id}/rerun-failed-jobs`, { method: "POST", body: {} }),
  rerunJob: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/jobs/${id}/rerun`, { method: "POST", body: {} }),
  deleteRun: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/runs/${id}`, { method: "DELETE" }),
  deleteRunLogs: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/runs/${id}/logs`, { method: "DELETE" }),
  /** API URLs that redirect to signed downloads (same origin, so a plain link works with the session). */
  runLogsUrl: (o: string, r: string, id: number, attempt?: number) =>
    `/api/v3${rp(o, r)}/actions/runs/${id}${attempt ? `/attempts/${attempt}` : ""}/logs`,
  jobLogsUrl: (o: string, r: string, id: number) => `/api/v3${rp(o, r)}/actions/jobs/${id}/logs`,
  pendingDeployments: (o: string, r: string, id: number) => request<PendingDeployment[]>(`${rp(o, r)}/actions/runs/${id}/pending_deployments`),
  reviewDeployments: (o: string, r: string, id: number, b: { environment_ids: number[]; state: "approved" | "rejected"; comment: string }) =>
    request<unknown>(`${rp(o, r)}/actions/runs/${id}/pending_deployments`, { method: "POST", body: b }),
  approvals: (o: string, r: string, id: number) => request<Approval[]>(`${rp(o, r)}/actions/runs/${id}/approvals`),

  // artifacts and caches
  runArtifacts: (o: string, r: string, id: number) => request<{ total_count: number; artifacts: Artifact[] }>(`${rp(o, r)}/actions/runs/${id}/artifacts`, { query: { per_page: 100 } }),
  artifactZipUrl: (o: string, r: string, id: number) => `/api/v3${rp(o, r)}/actions/artifacts/${id}/zip`,
  deleteArtifact: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/artifacts/${id}`, { method: "DELETE" }),
  caches: (o: string, r: string, q: { page?: number; key?: string; ref?: string; sort?: string; direction?: string }) =>
    requestPage<{ total_count: number; actions_caches: Cache[] }>(`${rp(o, r)}/actions/caches`, { query: { per_page: 30, ...q } }),
  deleteCache: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/actions/caches/${id}`, { method: "DELETE" }),
  cacheUsage: (o: string, r: string) => request<{ active_caches_size_in_bytes: number; active_caches_count: number }>(`${rp(o, r)}/actions/cache/usage`),

  // checks and statuses
  checkRuns: (o: string, r: string, ref: string) =>
    request<{ total_count: number; check_runs: CheckRun[] }>(`${rp(o, r)}/commits/${enc(ref)}/check-runs`, { query: { per_page: 100, filter: "latest" } }),
  suiteCheckRuns: (o: string, r: string, suite: number) =>
    request<{ total_count: number; check_runs: CheckRun[] }>(`${rp(o, r)}/check-suites/${suite}/check-runs`, { query: { per_page: 100 } }),
  checkRun: (o: string, r: string, id: number) => request<CheckRun>(`${rp(o, r)}/check-runs/${id}`),
  annotations: (o: string, r: string, id: number) => request<Annotation[]>(`${rp(o, r)}/check-runs/${id}/annotations`, { query: { per_page: 100 } }),
  rerequestCheckRun: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/check-runs/${id}/rerequest`, { method: "POST" }),
  combinedStatus: (o: string, r: string, ref: string) => request<CombinedStatus>(`${rp(o, r)}/commits/${enc(ref)}/status`, { query: { per_page: 100 } }),

  // deployments
  deployments: (o: string, r: string, q: { environment?: string; page?: number }) =>
    requestPage<Deployment[]>(`${rp(o, r)}/deployments`, { query: { per_page: 30, ...q } }),
  deploymentStatuses: (o: string, r: string, id: number) => request<DeploymentStatus[]>(`${rp(o, r)}/deployments/${id}/statuses`, { query: { per_page: 10 } }),

  // environments
  environments: (o: string, r: string) => request<{ total_count: number; environments: Environment[] }>(`${rp(o, r)}/environments`, { query: { per_page: 100 } }),
  environment: (o: string, r: string, name: string) => request<Environment>(`${rp(o, r)}/environments/${enc(name)}`),
  putEnvironment: (o: string, r: string, name: string, b: {
    wait_timer?: number; prevent_self_review?: boolean; reviewers?: { type: string; id: number }[] | null;
    deployment_branch_policy?: { protected_branches: boolean; custom_branch_policies: boolean } | null; can_admins_bypass?: boolean;
  }) => request<Environment>(`${rp(o, r)}/environments/${enc(name)}`, { method: "PUT", body: b }),
  deleteEnvironment: (o: string, r: string, name: string) => request<void>(`${rp(o, r)}/environments/${enc(name)}`, { method: "DELETE" }),
  branchPolicies: (o: string, r: string, name: string) =>
    request<{ total_count: number; branch_policies: BranchPolicy[] }>(`${rp(o, r)}/environments/${enc(name)}/deployment-branch-policies`, { query: { per_page: 100 } }),
  addBranchPolicy: (o: string, r: string, name: string, b: { name: string; type: "branch" | "tag" }) =>
    request<BranchPolicy>(`${rp(o, r)}/environments/${enc(name)}/deployment-branch-policies`, { method: "POST", body: b }),
  deleteBranchPolicy: (o: string, r: string, name: string, id: number) =>
    request<void>(`${rp(o, r)}/environments/${enc(name)}/deployment-branch-policies/${id}`, { method: "DELETE" }),

  // secrets and variables (repo, env, org)
  secrets: (s: Scope) => request<{ total_count: number; secrets: Secret[] }>(`${scopeBase(s)}/secrets`, { query: { per_page: 100 } }),
  putSecret: async (s: Scope, name: string, value: string, extra: { visibility?: string; selected_repository_ids?: number[] } = {}) => {
    const key = await request<{ key_id: string; key: string }>(`${scopeBase(s)}/secrets/public-key`);
    return request<void>(`${scopeBase(s)}/secrets/${enc(name)}`, {
      method: "PUT",
      body: { encrypted_value: sealSecret(key.key, value), key_id: key.key_id, ...extra },
    });
  },
  deleteSecret: (s: Scope, name: string) => request<void>(`${scopeBase(s)}/secrets/${enc(name)}`, { method: "DELETE" }),
  variables: (s: Scope) => request<{ total_count: number; variables: Variable[] }>(`${scopeBase(s)}/variables`, { query: { per_page: 30 } }),
  createVariable: (s: Scope, b: { name: string; value: string; visibility?: string; selected_repository_ids?: number[] }) =>
    request<void>(`${scopeBase(s)}/variables`, { method: "POST", body: b }),
  updateVariable: (s: Scope, name: string, b: { name?: string; value?: string; visibility?: string; selected_repository_ids?: number[] }) =>
    request<void>(`${scopeBase(s)}/variables/${enc(name)}`, { method: "PATCH", body: b }),
  deleteVariable: (s: Scope, name: string) => request<void>(`${scopeBase(s)}/variables/${enc(name)}`, { method: "DELETE" }),
  orgItemRepos: (org: string, kind: "secrets" | "variables", name: string) =>
    request<{ total_count: number; repositories: Repo[] }>(`/orgs/${enc(org)}/actions/${kind}/${enc(name)}/repositories`, { query: { per_page: 100 } }),
  setOrgItemRepos: (org: string, kind: "secrets" | "variables", name: string, ids: number[]) =>
    request<void>(`/orgs/${enc(org)}/actions/${kind}/${enc(name)}/repositories`, { method: "PUT", body: { selected_repository_ids: ids } }),
  orgRepos: (org: string) => request<Repo[]>(`/orgs/${enc(org)}/repos`, { query: { per_page: 100 } }),
  repoOrgSecrets: (o: string, r: string) => request<{ total_count: number; secrets: Secret[] }>(`${rp(o, r)}/actions/organization-secrets`, { query: { per_page: 100 } }),
  repoOrgVariables: (o: string, r: string) => request<{ total_count: number; variables: Variable[] }>(`${rp(o, r)}/actions/organization-variables`, { query: { per_page: 30 } }),

  // runners (repo, org, enterprise)
  runners: (s: Scope) => request<{ total_count: number; runners: Runner[] }>(`${scopeBase(s)}/runners`, { query: { per_page: 100 } }),
  deleteRunner: (s: Scope, id: number) => request<void>(`${scopeBase(s)}/runners/${id}`, { method: "DELETE" }),
  registrationToken: (s: Scope) => request<{ token: string; expires_at: string }>(`${scopeBase(s)}/runners/registration-token`, { method: "POST" }),
  removeToken: (s: Scope) => request<{ token: string; expires_at: string }>(`${scopeBase(s)}/runners/remove-token`, { method: "POST" }),
  addRunnerLabels: (s: Scope, id: number, labels: string[]) => request<unknown>(`${scopeBase(s)}/runners/${id}/labels`, { method: "POST", body: { labels } }),
  removeRunnerLabel: (s: Scope, id: number, name: string) => request<unknown>(`${scopeBase(s)}/runners/${id}/labels/${enc(name)}`, { method: "DELETE" }),
  runnerDownloads: (s: Scope) => request<{ os: string; architecture: string; download_url: string; filename: string }[]>(`${scopeBase(s)}/runners/downloads`),

  // runner groups (org, enterprise)
  runnerGroups: (s: Scope) => request<{ total_count: number; runner_groups: RunnerGroup[] }>(`${scopeBase(s)}/runner-groups`, { query: { per_page: 100 } }),
  createRunnerGroup: (s: Scope, b: Record<string, unknown>) => request<RunnerGroup>(`${scopeBase(s)}/runner-groups`, { method: "POST", body: b }),
  runnerGroup: (s: Scope, id: number) => request<RunnerGroup>(`${scopeBase(s)}/runner-groups/${id}`),
  updateRunnerGroup: (s: Scope, id: number, b: Record<string, unknown>) => request<RunnerGroup>(`${scopeBase(s)}/runner-groups/${id}`, { method: "PATCH", body: b }),
  deleteRunnerGroup: (s: Scope, id: number) => request<void>(`${scopeBase(s)}/runner-groups/${id}`, { method: "DELETE" }),
  runnerGroupRunners: (s: Scope, id: number) => request<{ total_count: number; runners: Runner[] }>(`${scopeBase(s)}/runner-groups/${id}/runners`, { query: { per_page: 100 } }),
  setRunnerGroupRunners: (s: Scope, id: number, ids: number[]) => request<void>(`${scopeBase(s)}/runner-groups/${id}/runners`, { method: "PUT", body: { runners: ids } }),
  runnerGroupRepos: (s: Scope, id: number) => request<{ total_count: number; repositories: Repo[] }>(`${scopeBase(s)}/runner-groups/${id}/repositories`, { query: { per_page: 100 } }),
  setRunnerGroupRepos: (s: Scope, id: number, ids: number[]) => request<void>(`${scopeBase(s)}/runner-groups/${id}/repositories`, { method: "PUT", body: { selected_repository_ids: ids } }),
  runnerGroupOrgs: (s: Scope, id: number) => request<{ total_count: number; organizations: OrgSummary[] }>(`${scopeBase(s)}/runner-groups/${id}/organizations`, { query: { per_page: 100 } }),
  setRunnerGroupOrgs: (s: Scope, id: number, ids: number[]) => request<void>(`${scopeBase(s)}/runner-groups/${id}/organizations`, { method: "PUT", body: { selected_organization_ids: ids } }),

  // permissions and settings (repo, org, enterprise): `${base}/permissions...`
  perm: <T>(s: Scope, sub = "") => request<T>(`${scopeBase(s)}/permissions${sub}`),
  putPerm: (s: Scope, sub: string, b: unknown) => request<void>(`${scopeBase(s)}/permissions${sub}`, { method: "PUT", body: b }),
  oidc: (s: Scope) => request<OidcSub>(`${scopeBase(s)}/oidc/customization/sub`),
  putOidc: (s: Scope, b: OidcSub) => request<unknown>(`${scopeBase(s)}/oidc/customization/sub`, { method: "PUT", body: b }),
  allOrgs: () => request<OrgSummary[]>("/organizations", { query: { per_page: 100 } }),
  userOrgs: () => request<OrgSummary[]>("/user/orgs", { query: { per_page: 100 } }),
};

export type { Page };
