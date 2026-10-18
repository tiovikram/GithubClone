// Rule types of repository rulesets (GHES 3.22) and their parameter editors' metadata.
export type ParamKind =
  | { kind: "bool"; key: string; label: string; desc?: string; default?: boolean }
  | { kind: "number"; key: string; label: string; desc?: string; default: number; min?: number; max?: number }
  | { kind: "select"; key: string; label: string; options: [string, string][]; default: string }
  | { kind: "multi"; key: string; label: string; options: [string, string][]; default: string[] }
  | { kind: "list"; key: string; label: string; desc?: string; placeholder?: string }
  | { kind: "text"; key: string; label: string; placeholder?: string; default?: string }
  | { kind: "checks" } // required_status_checks list of {context, integration_id}
  | { kind: "workflows" } // workflows [{path, repository_id, ref}]
  | { kind: "tools" }; // code scanning tools

export interface RuleMeta {
  type: string;
  label: string;
  desc: string;
  targets: ("branch" | "tag" | "push")[];
  params?: ParamKind[];
  /** Pre-checked when creating a ruleset of this target. */
  defaultOn?: ("branch" | "tag" | "push")[];
}

const patternParams: ParamKind[] = [
  { kind: "text", key: "name", label: "Name (description of the requirement)", placeholder: "e.g. Conventional commits" },
  { kind: "select", key: "operator", label: "Requirement", default: "starts_with",
    options: [["starts_with", "Must start with a matching pattern"], ["ends_with", "Must end with a matching pattern"], ["contains", "Must contain a matching pattern"], ["regex", "Must match a given regex pattern"]] },
  { kind: "text", key: "pattern", label: "Matching pattern", placeholder: "pattern" },
  { kind: "bool", key: "negate", label: "Negate (must not match)" },
];

export const RULES: RuleMeta[] = [
  { type: "creation", label: "Restrict creations", desc: "Only allow users with bypass permission to create matching refs.", targets: ["branch", "tag"] },
  { type: "update", label: "Restrict updates", desc: "Only allow users with bypass permission to update matching refs.", targets: ["branch", "tag"],
    params: [{ kind: "bool", key: "update_allows_fetch_and_merge", label: "Allow fetch and merge (fork syncing)" }] },
  { type: "deletion", label: "Restrict deletions", desc: "Only allow users with bypass permissions to delete matching refs.", targets: ["branch", "tag"], defaultOn: ["branch", "tag"] },
  { type: "required_linear_history", label: "Require linear history", desc: "Prevent merge commits from being pushed to matching refs.", targets: ["branch", "tag"] },
  { type: "merge_queue", label: "Require merge queue", desc: "Merges must be performed via a merge queue.", targets: ["branch"],
    params: [
      { kind: "select", key: "merge_method", label: "Merge method", default: "MERGE", options: [["MERGE", "Merge commit"], ["SQUASH", "Squash and merge"], ["REBASE", "Rebase and merge"]] },
      { kind: "number", key: "max_entries_to_build", label: "Build concurrency", default: 5, min: 0, max: 100 },
      { kind: "number", key: "min_entries_to_merge", label: "Minimum group size", default: 1, min: 0, max: 100 },
      { kind: "number", key: "max_entries_to_merge", label: "Maximum group size", default: 5, min: 0, max: 100 },
      { kind: "number", key: "min_entries_to_merge_wait_minutes", label: "Wait time to meet minimum group size (minutes)", default: 5, min: 0, max: 360 },
      { kind: "select", key: "grouping_strategy", label: "Require all queue entries to pass required checks", default: "ALLGREEN", options: [["ALLGREEN", "All entries"], ["HEADGREEN", "Only the head entry"]] },
      { kind: "number", key: "check_response_timeout_minutes", label: "Status check timeout (minutes)", default: 60, min: 1, max: 360 },
    ] },
  { type: "required_deployments", label: "Require deployments to succeed", desc: "Choose which environments must be successfully deployed to before refs can be pushed into a ref that matches this rule.", targets: ["branch", "tag"],
    params: [{ kind: "list", key: "required_deployment_environments", label: "Environments (one per line)", placeholder: "production" }] },
  { type: "required_signatures", label: "Require signed commits", desc: "Commits pushed to matching refs must have verified signatures.", targets: ["branch", "tag"] },
  { type: "pull_request", label: "Require a pull request before merging", desc: "Require all commits be made to a non-target branch and submitted via a pull request before they can be merged.", targets: ["branch"],
    params: [
      { kind: "number", key: "required_approving_review_count", label: "Required approvals", default: 1, min: 0, max: 10 },
      { kind: "bool", key: "dismiss_stale_reviews_on_push", label: "Dismiss stale pull request approvals when new commits are pushed" },
      { kind: "bool", key: "require_code_owner_review", label: "Require review from Code Owners" },
      { kind: "bool", key: "require_last_push_approval", label: "Require approval of the most recent reviewable push" },
      { kind: "bool", key: "required_review_thread_resolution", label: "Require conversation resolution before merging" },
      { kind: "multi", key: "allowed_merge_methods", label: "Allowed merge methods", default: ["merge", "squash", "rebase"], options: [["merge", "Merge"], ["squash", "Squash"], ["rebase", "Rebase"]] },
    ] },
  { type: "required_status_checks", label: "Require status checks to pass", desc: "Choose which status checks must pass before the ref is updated.", targets: ["branch", "tag"],
    params: [
      { kind: "bool", key: "strict_required_status_checks_policy", label: "Require branches to be up to date before merging" },
      { kind: "bool", key: "do_not_enforce_on_create", label: "Do not require status checks on creation" },
      { kind: "checks" },
    ] },
  { type: "non_fast_forward", label: "Block force pushes", desc: "Prevent users with push access from force pushing to refs.", targets: ["branch", "tag"], defaultOn: ["branch", "tag"] },
  { type: "workflows", label: "Require workflows to pass before merging", desc: "Require all changes made to a targeted branch to pass the specified workflows before they can be merged.", targets: ["branch"],
    params: [{ kind: "bool", key: "do_not_enforce_on_create", label: "Do not require workflows on creation" }, { kind: "workflows" }] },
  { type: "code_scanning", label: "Require code scanning results", desc: "Choose which tools must provide code scanning results before the reference is updated.", targets: ["branch"],
    params: [{ kind: "tools" }] },
  { type: "commit_message_pattern", label: "Restrict commit metadata: commit message", desc: "Commit messages must match the pattern.", targets: ["branch", "tag"], params: patternParams },
  { type: "commit_author_email_pattern", label: "Restrict commit metadata: author email", desc: "Commit author emails must match the pattern.", targets: ["branch", "tag"], params: patternParams },
  { type: "committer_email_pattern", label: "Restrict commit metadata: committer email", desc: "Committer emails must match the pattern.", targets: ["branch", "tag"], params: patternParams },
  { type: "branch_name_pattern", label: "Restrict branch names", desc: "Branch names must match the pattern.", targets: ["branch"], params: patternParams },
  { type: "tag_name_pattern", label: "Restrict tag names", desc: "Tag names must match the pattern.", targets: ["tag"], params: patternParams },
  { type: "file_path_restriction", label: "Restrict file paths", desc: "Prevent commits that include changes in specified file paths from being pushed.", targets: ["push"],
    params: [{ kind: "list", key: "restricted_file_paths", label: "File paths (one per line, fnmatch syntax)", placeholder: "secrets/**" }] },
  { type: "max_file_path_length", label: "Restrict file path length", desc: "Prevent commits that include file paths that exceed a specified character limit from being pushed.", targets: ["push"],
    params: [{ kind: "number", key: "max_file_path_length", label: "Maximum path length", default: 255, min: 1, max: 32767 }] },
  { type: "file_extension_restriction", label: "Restrict file extensions", desc: "Prevent commits that include files with specified file extensions from being pushed.", targets: ["push"],
    params: [{ kind: "list", key: "restricted_file_extensions", label: "Extensions (one per line)", placeholder: ".exe" }] },
  { type: "max_file_size", label: "Restrict file size", desc: "Prevent commits that exceed a specified file size limit from being pushed.", targets: ["push"],
    params: [{ kind: "number", key: "max_file_size", label: "Maximum file size (MB)", default: 100, min: 1, max: 100 }] },
];

export const ruleLabel = (t: string) => RULES.find((r) => r.type === t)?.label || t;

/** Default parameters for a newly enabled rule. */
export function defaultParams(m: RuleMeta): Record<string, unknown> | undefined {
  if (!m.params) return undefined;
  const p: Record<string, unknown> = {};
  for (const x of m.params) {
    if (x.kind === "bool") p[x.key] = x.default ?? false;
    else if (x.kind === "number") p[x.key] = x.default;
    else if (x.kind === "select") p[x.key] = x.default;
    else if (x.kind === "multi") p[x.key] = [...x.default];
    else if (x.kind === "list") p[x.key] = [];
    else if (x.kind === "text") p[x.key] = x.default ?? "";
    else if (x.kind === "checks") p.required_status_checks = [];
    else if (x.kind === "workflows") p.workflows = [];
    else if (x.kind === "tools") p.code_scanning_tools = [{ tool: "CodeQL", alerts_threshold: "errors", security_alerts_threshold: "high_or_higher" }];
  }
  return p;
}

export const ACTOR_LABEL: Record<string, string> = {
  OrganizationAdmin: "Organization admin",
  EnterpriseOwner: "Enterprise owner",
  DeployKey: "Deploy keys",
  RepositoryRole: "Repository role",
  Team: "Team",
  Integration: "App",
  User: "User",
};

/** GitHub's base repository role ids for `RepositoryRole` bypass actors. */
export const ROLE_IDS: [number, string][] = [[1, "Read"], [3, "Triage"], [4, "Write"], [2, "Maintain"], [5, "Repository admin"]];

export const MODES: [string, string][] = [["always", "Always allow"], ["pull_request", "For pull requests only"], ["exempt", "Exempt"]];
