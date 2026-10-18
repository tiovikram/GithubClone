// Endpoint helpers and types for the security area (GitHub REST shapes; only fields the UI uses).
import { ApiError, request, requestAll, requestPage, type Page, type Repo, type User } from "../../api";

const enc = encodeURIComponent;
const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;
type Q = Record<string, string | number | boolean | undefined | null>;

/** The enterprise slug of this install (FORGE_ENTERPRISE_SLUG, default "forge"). */
export const ENTERPRISE = "forge";

// ---------- types ----------

export type Severity = "critical" | "high" | "medium" | "low" | string;

export interface Cwe { cwe_id: string; name: string }
export interface Identifier { type: string; value: string }

export interface GlobalAdvisory {
  ghsa_id: string;
  cve_id: string | null;
  html_url: string;
  summary: string;
  description: string | null;
  type: "reviewed" | "unreviewed" | "malware" | string;
  severity: Severity;
  source_code_location: string | null;
  identifiers: Identifier[];
  references: string[];
  published_at: string;
  updated_at: string;
  github_reviewed_at: string | null;
  nvd_published_at: string | null;
  withdrawn_at: string | null;
  vulnerabilities: {
    package: { ecosystem: string; name: string };
    vulnerable_version_range: string | null;
    first_patched_version: string | null;
    vulnerable_functions: string[];
  }[];
  cvss: { vector_string: string | null; score: number | null } | null;
  cwes: Cwe[];
}

export interface RepoAdvisoryVuln {
  package: { ecosystem: string; name: string | null };
  vulnerable_version_range: string | null;
  patched_versions: string | null;
  vulnerable_functions: string[] | null;
}

export interface RepoAdvisory {
  ghsa_id: string;
  cve_id: string | null;
  html_url: string;
  summary: string;
  description: string | null;
  severity: Severity | null;
  author: User | null;
  publisher: User | null;
  identifiers: Identifier[];
  state: "triage" | "draft" | "published" | "closed" | "withdrawn";
  created_at: string;
  updated_at: string;
  published_at: string | null;
  closed_at: string | null;
  withdrawn_at: string | null;
  submission: { accepted: boolean } | null;
  vulnerabilities: RepoAdvisoryVuln[];
  cvss: { vector_string: string | null; score: number | null } | null;
  cwes: Cwe[];
  cwe_ids: string[];
  credits: { login: string; type: string }[];
  credits_detailed: { user: User; type: string; state: string }[];
  collaborating_users: User[];
  collaborating_teams: { id: number; name: string; slug: string }[];
  cve_requested_at?: string | null;
}

export interface RepoAdvisoryBody {
  summary?: string;
  description?: string;
  cve_id?: string | null;
  vulnerabilities?: RepoAdvisoryVuln[];
  cwe_ids?: string[];
  credits?: { login: string; type: string }[];
  severity?: string | null;
  cvss_vector_string?: string | null;
  state?: string;
  collaborating_users?: string[];
}

export interface DependabotAlert {
  number: number;
  state: "open" | "dismissed" | "fixed" | "auto_dismissed";
  dependency: { package: { ecosystem: string; name: string }; manifest_path: string; scope: "development" | "runtime" | null };
  security_advisory: {
    ghsa_id: string; cve_id: string | null; summary: string; description: string; severity: Severity;
    identifiers: Identifier[]; references: { url: string }[]; published_at: string; cwes: Cwe[];
    cvss: { score: number; vector_string: string | null };
    vulnerabilities: { package: { ecosystem: string; name: string }; vulnerable_version_range: string; first_patched_version: { identifier: string } | null }[];
  };
  security_vulnerability: { package: { ecosystem: string; name: string }; severity: Severity; vulnerable_version_range: string; first_patched_version: { identifier: string } | null };
  html_url: string;
  created_at: string;
  updated_at: string;
  dismissed_at: string | null;
  dismissed_by: User | null;
  dismissed_reason: string | null;
  dismissed_comment: string | null;
  fixed_at: string | null;
  auto_dismissed_at: string | null;
  repository?: { full_name: string; name: string; owner: { login: string } };
}

export interface CodeScanningAlert {
  number: number;
  state: "open" | "dismissed" | "fixed" | "closed";
  html_url: string;
  created_at: string;
  updated_at: string | null;
  fixed_at: string | null;
  dismissed_by: User | null;
  dismissed_at: string | null;
  dismissed_reason: string | null;
  dismissed_comment: string | null;
  rule: {
    id: string; name: string; severity: "none" | "note" | "warning" | "error" | null;
    security_severity_level: "low" | "medium" | "high" | "critical" | null;
    description: string; full_description?: string; tags: string[] | null; help?: string | null; help_uri?: string | null;
  };
  tool: { name: string; guid: string | null; version: string | null };
  most_recent_instance: CodeScanningInstance;
  repository?: { full_name: string };
}

export interface CodeScanningInstance {
  ref: string;
  analysis_key: string;
  environment: string;
  category: string;
  state: string;
  commit_sha: string;
  message: { text: string };
  location: { path: string; start_line: number; end_line: number; start_column: number; end_column: number };
  html_url?: string;
  classifications: string[];
}

export interface CodeScanningAnalysis {
  id: number;
  ref: string;
  commit_sha: string;
  analysis_key: string;
  environment: string;
  category: string;
  error: string;
  created_at: string;
  results_count: number;
  rules_count: number;
  sarif_id: string;
  tool: { name: string; guid: string | null; version: string | null };
  deletable: boolean;
  warning: string;
}

export interface DefaultSetup {
  state: "configured" | "not-configured";
  languages: string[];
  runner_type: string | null;
  runner_label: string | null;
  query_suite: "default" | "extended";
  threat_model?: "remote" | "remote_and_local";
  updated_at: string | null;
  schedule: string | null;
}

export interface SecretLocation {
  type: string;
  details: { path?: string; start_line?: number; end_line?: number; start_column?: number; end_column?: number; commit_sha?: string; html_url?: string; blob_sha?: string };
}

export interface SecretAlert {
  number: number;
  created_at: string;
  updated_at: string | null;
  html_url: string;
  state: "open" | "resolved";
  resolution: "false_positive" | "wont_fix" | "revoked" | "used_in_tests" | null;
  resolved_at: string | null;
  resolved_by: User | null;
  resolution_comment: string | null;
  secret_type: string;
  secret_type_display_name: string;
  provider: string | null;
  secret?: string;
  push_protection_bypassed: boolean | null;
  push_protection_bypassed_by: User | null;
  push_protection_bypassed_at: string | null;
  push_protection_bypass_request_comment: string | null;
  push_protection_bypass_request_html_url: string | null;
  validity: "active" | "inactive" | "unknown";
  first_location_detected: SecretLocation["details"] | null;
  has_more_locations: boolean;
  closure_request_comment?: string | null;
}

export interface SecurityRequest {
  id: number;
  number: number;
  repository: { id: number; name: string; full_name: string };
  requester: { actor_id: number; actor_name: string } | null;
  request_type: string;
  data: Record<string, unknown>[] | null;
  resource_identifier: string;
  status: "pending" | "approved" | "denied" | "rejected" | "expired" | "cancelled" | "completed" | string;
  requester_comment: string | null;
  expires_at: string;
  created_at: string;
  responses: { id: number; reviewer: { actor_id: number; actor_name: string } | null; message: string | null; status: string; created_at: string }[] | null;
  html_url: string;
}

export interface ScanHistory {
  incremental_scans: { type: string; status: string; completed_at: string | null; started_at?: string | null }[];
  backfill_scans: { type: string; status: string; completed_at: string | null; started_at?: string | null }[];
  pattern_update_scans?: unknown[];
  custom_pattern_backfill_scans?: unknown[];
}

export type Status = "enabled" | "disabled";
export interface SecurityAndAnalysis {
  advanced_security?: { status: Status };
  dependabot_security_updates?: { status: Status };
  secret_scanning?: { status: Status };
  secret_scanning_push_protection?: { status: Status };
  secret_scanning_non_provider_patterns?: { status: Status };
  secret_scanning_validity_checks?: { status: Status };
  secret_scanning_delegated_alert_dismissal?: { status: Status };
  secret_scanning_delegated_bypass?: { status: Status };
  secret_scanning_delegated_bypass_options?: { reviewers: { reviewer_id: number; reviewer_type: "TEAM" | "ROLE" }[] };
}
export type RepoWithSecurity = Repo & { security_and_analysis?: SecurityAndAnalysis };

export type TriState = "enabled" | "disabled" | "not_set";
export interface CodeSecurityConfig {
  id: number;
  name: string;
  target_type: "global" | "organization" | "enterprise";
  description: string;
  advanced_security: TriState | "code_security" | "secret_protection";
  dependency_graph: TriState;
  dependency_graph_autosubmit_action: TriState;
  dependabot_alerts: TriState;
  dependabot_security_updates: TriState;
  dependabot_delegated_alert_dismissal?: TriState;
  code_scanning_default_setup: TriState;
  code_scanning_delegated_alert_dismissal?: TriState;
  secret_scanning: TriState;
  secret_scanning_push_protection: TriState;
  secret_scanning_delegated_bypass: TriState;
  secret_scanning_validity_checks: TriState;
  secret_scanning_non_provider_patterns: TriState;
  secret_scanning_delegated_alert_dismissal?: TriState;
  private_vulnerability_reporting: TriState;
  enforcement: "enforced" | "unenforced";
  html_url?: string;
  created_at: string;
  updated_at: string;
  [k: string]: unknown;
}
export interface ConfigDefault { default_for_new_repos: "all" | "none" | "private_and_internal" | "public"; configuration: CodeSecurityConfig }
export interface ConfigRepo { status: string; repository: Repo }

export interface DependencyChange {
  change_type: "added" | "removed";
  manifest: string;
  ecosystem: string;
  name: string;
  version: string;
  package_url: string | null;
  license: string | null;
  source_repository_url: string | null;
  scope: "runtime" | "development" | "unknown";
  vulnerabilities: { severity: Severity; advisory_ghsa_id: string; advisory_summary: string; advisory_url: string }[];
}

export interface SbomPackage {
  SPDXID: string;
  name: string;
  versionInfo: string;
  externalRefs: { referenceCategory: string; referenceType: string; referenceLocator: string }[];
}
export interface Sbom { sbom: { name: string; spdxVersion: string; creationInfo: { created: string }; packages: SbomPackage[]; relationships: unknown[] } }

export interface PublicKey { key_id: string; key: string }
export interface DepSecret { name: string; created_at: string; updated_at: string; visibility?: "all" | "private" | "selected"; selected_repositories_url?: string }

export interface Team { id: number; name: string; slug: string }
export interface OrgRole { id: number; name: string }

// ---------- helpers ----------

/** 403/404 from an alerts API = feature disabled or no access. */
export const isOff = (e: unknown) => e instanceof ApiError && (e.status === 403 || e.status === 404);

async function page<T>(path: string, query: Q) {
  return requestPage<T[]>(path, { query });
}

export const sec = {
  // repo
  repo: (o: string, r: string) => request<RepoWithSecurity>(rp(o, r)),
  setSecurity: (o: string, r: string, sa: Record<string, unknown>) =>
    request<RepoWithSecurity>(rp(o, r), { method: "PATCH", body: { security_and_analysis: sa } }),
  vulnAlerts: async (o: string, r: string) => {
    try {
      await request<void>(`${rp(o, r)}/vulnerability-alerts`);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return false;
      throw e;
    }
  },
  setVulnAlerts: (o: string, r: string, on: boolean) => request<void>(`${rp(o, r)}/vulnerability-alerts`, { method: on ? "PUT" : "DELETE" }),
  autoFixes: (o: string, r: string) => request<{ enabled: boolean; paused: boolean }>(`${rp(o, r)}/automated-security-fixes`).catch((e) => {
    if (e instanceof ApiError && e.status === 404) return { enabled: false, paused: false };
    throw e;
  }),
  setAutoFixes: (o: string, r: string, on: boolean) => request<void>(`${rp(o, r)}/automated-security-fixes`, { method: on ? "PUT" : "DELETE" }),
  pvr: (o: string, r: string) => request<{ enabled: boolean }>(`${rp(o, r)}/private-vulnerability-reporting`),
  setPvr: (o: string, r: string, on: boolean) => request<void>(`${rp(o, r)}/private-vulnerability-reporting`, { method: on ? "PUT" : "DELETE" }),
  repoConfig: (o: string, r: string) =>
    request<{ status: string; configuration: CodeSecurityConfig } | undefined>(`${rp(o, r)}/code-security-configuration`).catch((e) => {
      if (e instanceof ApiError && (e.status === 404 || e.status === 403)) return undefined;
      throw e;
    }),

  // repository advisories
  advisories: (o: string, r: string, state?: string) =>
    requestAll<RepoAdvisory>(`${rp(o, r)}/security-advisories`, { query: { state, sort: "updated" } }, 5),
  advisory: (o: string, r: string, ghsa: string) => request<RepoAdvisory>(`${rp(o, r)}/security-advisories/${enc(ghsa)}`),
  createAdvisory: (o: string, r: string, b: RepoAdvisoryBody) => request<RepoAdvisory>(`${rp(o, r)}/security-advisories`, { method: "POST", body: b }),
  reportVuln: (o: string, r: string, b: RepoAdvisoryBody) => request<RepoAdvisory>(`${rp(o, r)}/security-advisories/reports`, { method: "POST", body: b }),
  updateAdvisory: (o: string, r: string, ghsa: string, b: RepoAdvisoryBody) =>
    request<RepoAdvisory>(`${rp(o, r)}/security-advisories/${enc(ghsa)}`, { method: "PATCH", body: b }),
  requestCve: (o: string, r: string, ghsa: string) => request<unknown>(`${rp(o, r)}/security-advisories/${enc(ghsa)}/cve`, { method: "POST" }),
  orgAdvisories: (org: string) => requestAll<RepoAdvisory & { html_url: string }>(`/orgs/${enc(org)}/security-advisories`, {}, 3),

  // dependabot
  dependabotAlerts: (o: string, r: string, q: Q) => page<DependabotAlert>(`${rp(o, r)}/dependabot/alerts`, { per_page: 25, ...q }),
  dependabotAlert: (o: string, r: string, n: number) => request<DependabotAlert>(`${rp(o, r)}/dependabot/alerts/${n}`),
  updateDependabot: (o: string, r: string, n: number, b: { state: string; dismissed_reason?: string; dismissed_comment?: string }) =>
    request<DependabotAlert>(`${rp(o, r)}/dependabot/alerts/${n}`, { method: "PATCH", body: b }),
  orgDependabot: (org: string, q: Q) => page<DependabotAlert>(`/orgs/${enc(org)}/dependabot/alerts`, { per_page: 50, ...q }),

  // dismissal requests (kind: dependabot | code-scanning | secret-scanning)
  dismissalRequests: (o: string, r: string, kind: string) => requestAll<SecurityRequest>(`${rp(o, r)}/dismissal-requests/${kind}`, {}, 3),
  dismissalRequest: (o: string, r: string, kind: string, n: number) =>
    request<SecurityRequest>(`${rp(o, r)}/dismissal-requests/${kind}/${n}`).catch((e) => {
      if (e instanceof ApiError && e.status === 404) return undefined;
      throw e;
    }),
  createDependabotRequest: (o: string, r: string, n: number, b: { dismissed_reason: string; dismissed_comment?: string }) =>
    request<SecurityRequest>(`${rp(o, r)}/dismissal-requests/dependabot/${n}`, { method: "POST", body: b }),
  cancelDependabotRequest: (o: string, r: string, n: number) => request<void>(`${rp(o, r)}/dismissal-requests/dependabot/${n}`, { method: "DELETE" }),
  reviewDismissal: (o: string, r: string, kind: string, n: number, status: "approve" | "deny", message: string) =>
    request<unknown>(`${rp(o, r)}/dismissal-requests/${kind}/${n}`, { method: "PATCH", body: { status, message } }),
  orgDismissalRequests: (org: string, kind: string) => requestAll<SecurityRequest>(`/orgs/${enc(org)}/dismissal-requests/${kind}`, {}, 3),

  // code scanning
  codeAlerts: (o: string, r: string, q: Q) => page<CodeScanningAlert>(`${rp(o, r)}/code-scanning/alerts`, { per_page: 25, ...q }),
  codeAlert: (o: string, r: string, n: number) => request<CodeScanningAlert>(`${rp(o, r)}/code-scanning/alerts/${n}`),
  codeInstances: (o: string, r: string, n: number) => request<CodeScanningInstance[]>(`${rp(o, r)}/code-scanning/alerts/${n}/instances`),
  updateCode: (o: string, r: string, n: number, b: { state: string; dismissed_reason?: string; dismissed_comment?: string; create_request?: boolean }) =>
    request<CodeScanningAlert>(`${rp(o, r)}/code-scanning/alerts/${n}`, { method: "PATCH", body: b }),
  analyses: (o: string, r: string, q: Q = {}) => request<CodeScanningAnalysis[]>(`${rp(o, r)}/code-scanning/analyses`, { query: { per_page: 100, ...q } }),
  deleteAnalysis: (o: string, r: string, id: number, confirm: boolean) =>
    request<{ next_analysis_url: string | null; confirm_delete_url: string | null }>(`${rp(o, r)}/code-scanning/analyses/${id}`, { method: "DELETE", query: { confirm_delete: confirm ? "true" : undefined } }),
  uploadSarif: (o: string, r: string, b: { commit_sha: string; ref: string; sarif: string; tool_name?: string }) =>
    request<{ id: string; url: string }>(`${rp(o, r)}/code-scanning/sarifs`, { method: "POST", body: b }),
  sarifStatus: (o: string, r: string, id: string) =>
    request<{ processing_status: "pending" | "complete" | "failed"; analyses_url: string | null; errors: string[] | null }>(`${rp(o, r)}/code-scanning/sarifs/${enc(id)}`),
  defaultSetup: (o: string, r: string) => request<DefaultSetup>(`${rp(o, r)}/code-scanning/default-setup`),
  setDefaultSetup: (o: string, r: string, b: { state: string; query_suite?: string; languages?: string[] }) =>
    request<unknown>(`${rp(o, r)}/code-scanning/default-setup`, { method: "PATCH", body: b }),
  orgCode: (org: string, q: Q) => page<CodeScanningAlert>(`/orgs/${enc(org)}/code-scanning/alerts`, { per_page: 50, ...q }),

  // secret scanning
  secretAlerts: (o: string, r: string, q: Q) => page<SecretAlert>(`${rp(o, r)}/secret-scanning/alerts`, { per_page: 25, ...q }),
  secretAlert: (o: string, r: string, n: number) => request<SecretAlert>(`${rp(o, r)}/secret-scanning/alerts/${n}`),
  secretLocations: (o: string, r: string, n: number) => request<SecretLocation[]>(`${rp(o, r)}/secret-scanning/alerts/${n}/locations`, { query: { per_page: 100 } }),
  updateSecret: (o: string, r: string, n: number, b: { state: string; resolution?: string; resolution_comment?: string }) =>
    request<SecretAlert>(`${rp(o, r)}/secret-scanning/alerts/${n}`, { method: "PATCH", body: b }),
  bypass: (o: string, r: string, b: { reason: string; placeholder_id: string }) =>
    request<{ reason: string; expire_at: string; token_type: string }>(`${rp(o, r)}/secret-scanning/push-protection-bypasses`, { method: "POST", body: b }),
  scanHistory: (o: string, r: string) => request<ScanHistory>(`${rp(o, r)}/secret-scanning/scan-history`),
  bypassRequests: (o: string, r: string) => requestAll<SecurityRequest>(`${rp(o, r)}/bypass-requests/secret-scanning`, {}, 3),
  bypassRequest: (o: string, r: string, n: number) => request<SecurityRequest>(`${rp(o, r)}/bypass-requests/secret-scanning/${n}`),
  reviewBypass: (o: string, r: string, n: number, status: "approve" | "reject", message: string) =>
    request<unknown>(`${rp(o, r)}/bypass-requests/secret-scanning/${n}`, { method: "PATCH", body: { status, message } }),
  orgSecrets: (org: string, q: Q) => page<SecretAlert & { repository?: { full_name: string } }>(`/orgs/${enc(org)}/secret-scanning/alerts`, { per_page: 50, ...q }),
  orgBypassRequests: (org: string) => requestAll<SecurityRequest>(`/orgs/${enc(org)}/bypass-requests/secret-scanning`, {}, 3),
  orgPatterns: (org: string) => request<PatternConfig>(`/orgs/${enc(org)}/secret-scanning/pattern-configurations`),
  setOrgPatterns: (org: string, b: { pattern_config_version: string | null; provider_pattern_settings: { token_type: string; push_protection_setting: string }[] }) =>
    request<{ pattern_config_version: string }>(`/orgs/${enc(org)}/secret-scanning/pattern-configurations`, { method: "PATCH", body: b }),

  // dependency graph
  sbom: (o: string, r: string) => request<Sbom>(`${rp(o, r)}/dependency-graph/sbom`),
  depCompare: (o: string, r: string, base: string, head: string) =>
    request<DependencyChange[]>(`${rp(o, r)}/dependency-graph/compare/${enc(base)}...${enc(head)}`),

  // dependabot secrets (repo / org)
  repoSecretKey: (o: string, r: string) => request<PublicKey>(`${rp(o, r)}/dependabot/secrets/public-key`),
  repoSecrets: (o: string, r: string) => request<{ total_count: number; secrets: DepSecret[] }>(`${rp(o, r)}/dependabot/secrets`, { query: { per_page: 100 } }),
  putRepoSecret: (o: string, r: string, name: string, b: { encrypted_value: string; key_id: string }) =>
    request<unknown>(`${rp(o, r)}/dependabot/secrets/${enc(name)}`, { method: "PUT", body: b }),
  deleteRepoSecret: (o: string, r: string, name: string) => request<void>(`${rp(o, r)}/dependabot/secrets/${enc(name)}`, { method: "DELETE" }),
  orgSecretKey: (org: string) => request<PublicKey>(`/orgs/${enc(org)}/dependabot/secrets/public-key`),
  orgDepSecrets: (org: string) => request<{ total_count: number; secrets: DepSecret[] }>(`/orgs/${enc(org)}/dependabot/secrets`, { query: { per_page: 100 } }),
  putOrgSecret: (org: string, name: string, b: { encrypted_value: string; key_id: string; visibility: string; selected_repository_ids?: number[] }) =>
    request<unknown>(`/orgs/${enc(org)}/dependabot/secrets/${enc(name)}`, { method: "PUT", body: b }),
  deleteOrgSecret: (org: string, name: string) => request<void>(`/orgs/${enc(org)}/dependabot/secrets/${enc(name)}`, { method: "DELETE" }),
  orgSecretRepos: (org: string, name: string) =>
    request<{ total_count: number; repositories: Repo[] }>(`/orgs/${enc(org)}/dependabot/secrets/${enc(name)}/repositories`),
  orgRepos: (org: string) => requestAll<Repo>(`/orgs/${enc(org)}/repos`, { query: { per_page: 100 } }, 5),
  depAccess: (org: string) =>
    request<{ default_level: "public" | "internal" | null; accessible_repositories: Repo[] }>(`/orgs/${enc(org)}/dependabot/repository-access`),
  setDepAccess: (org: string, b: { repository_ids_to_add?: number[]; repository_ids_to_remove?: number[] }) =>
    request<void>(`/orgs/${enc(org)}/dependabot/repository-access`, { method: "PATCH", body: b }),
  setDepAccessDefault: (org: string, level: string) =>
    request<void>(`/orgs/${enc(org)}/dependabot/repository-access/default-level`, { method: "PUT", body: { default_level: level } }),

  // code security configurations (org / enterprise)
  configs: (scope: string) => request<CodeSecurityConfig[]>(`${scope}/code-security/configurations`, { query: { per_page: 100 } }),
  config: (scope: string, id: number) => request<CodeSecurityConfig>(`${scope}/code-security/configurations/${id}`),
  createConfig: (scope: string, b: Partial<CodeSecurityConfig>) => request<CodeSecurityConfig>(`${scope}/code-security/configurations`, { method: "POST", body: b }),
  updateConfig: (scope: string, id: number, b: Partial<CodeSecurityConfig>) =>
    request<CodeSecurityConfig>(`${scope}/code-security/configurations/${id}`, { method: "PATCH", body: b }),
  deleteConfig: (scope: string, id: number) => request<void>(`${scope}/code-security/configurations/${id}`, { method: "DELETE" }),
  attachConfig: (scope: string, id: number, b: { scope: string; selected_repository_ids?: number[] }) =>
    request<unknown>(`${scope}/code-security/configurations/${id}/attach`, { method: "POST", body: b }),
  setConfigDefault: (scope: string, id: number, d: string) =>
    request<unknown>(`${scope}/code-security/configurations/${id}/defaults`, { method: "PUT", body: { default_for_new_repos: d } }),
  configDefaults: (scope: string) => request<ConfigDefault[]>(`${scope}/code-security/configurations/defaults`),
  configRepos: (scope: string, id: number) => request<ConfigRepo[]>(`${scope}/code-security/configurations/${id}/repositories`, { query: { per_page: 100 } }),
  detachConfig: (org: string, ids: number[]) =>
    request<void>(`/orgs/${enc(org)}/code-security/configurations/detach`, { method: "DELETE", body: { selected_repository_ids: ids } }),
  orgEnableAll: (org: string, product: string, enablement: "enable_all" | "disable_all") =>
    request<void>(`/orgs/${enc(org)}/${product}/${enablement}`, { method: "POST" }),

  // enterprise
  entSettings: (e = ENTERPRISE) => request<EntCodeSecurity>(`/enterprises/${enc(e)}/code_security_and_analysis`),
  setEntSettings: (e: string, b: Partial<EntCodeSecurity>) => request<void>(`/enterprises/${enc(e)}/code_security_and_analysis`, { method: "PATCH", body: b }),
  entPatterns: (e = ENTERPRISE) => request<PatternConfig>(`/enterprises/${enc(e)}/secret-scanning/pattern-configurations`),

  // orgs (for reviewer pickers)
  orgTeams: (org: string) => requestAll<Team>(`/orgs/${enc(org)}/teams`, {}, 3),

  // global advisories
  globalAdvisories: (q: Q) => requestPage<GlobalAdvisory[]>("/advisories", { query: { per_page: 25, ...q } }),
  globalAdvisory: (ghsa: string) => request<GlobalAdvisory>(`/advisories/${enc(ghsa)}`),
};

export interface PatternConfig {
  pattern_config_version: string | null;
  provider_pattern_overrides: { token_type: string; slug: string; display_name: string; alert_total: number; alert_total_percentage: number; false_positives: number; false_positive_rate: number; bypass_rate: number; default_setting: string; setting: string; enterprise_setting?: string | null }[];
  custom_pattern_overrides: unknown[];
}

export interface EntCodeSecurity {
  advanced_security_enabled_for_new_repositories: boolean;
  dependabot_alerts_enabled_for_new_repositories: boolean;
  secret_scanning_enabled_for_new_repositories: boolean;
  secret_scanning_push_protection_enabled_for_new_repositories: boolean;
  secret_scanning_push_protection_custom_link: string | null;
  secret_scanning_non_provider_patterns_enabled_for_new_repositories?: boolean;
  secret_scanning_validity_checks_enabled?: boolean;
}

export type { Page };

// ---------- display helpers ----------

export const SEVERITY_ORDER: Severity[] = ["critical", "high", "medium", "low"];
export const severityLabel = (s: Severity | null | undefined) => (s === "medium" ? "Moderate" : s ? s[0].toUpperCase() + s.slice(1) : "Unknown");

export const DEPENDABOT_REASONS: [string, string][] = [
  ["fix_started", "A fix has already been started"],
  ["no_bandwidth", "No bandwidth to fix this"],
  ["tolerable_risk", "Risk is tolerable to this project"],
  ["inaccurate", "This alert is inaccurate or incorrect"],
  ["not_used", "Vulnerable code is not actually used"],
];
export const CODE_REASONS: [string, string][] = [
  ["false positive", "False positive"],
  ["used in tests", "Used in tests"],
  ["won't fix", "Won't fix"],
];
export const SECRET_RESOLUTIONS: [string, string][] = [
  ["false_positive", "False positive"],
  ["used_in_tests", "Used in tests"],
  ["wont_fix", "Won't fix"],
  ["revoked", "Revoked"],
];
export const BYPASS_REASONS: [string, string][] = [
  ["used_in_tests", "It's used in tests"],
  ["false_positive", "It's a false positive"],
  ["will_fix_later", "I'll fix it later"],
];
export const ECOSYSTEMS: [string, string][] = [
  ["npm", "npm"], ["pip", "pip"], ["maven", "Maven"], ["nuget", "NuGet"], ["composer", "Composer"], ["go", "Go"],
  ["rust", "Rust"], ["rubygems", "RubyGems"], ["erlang", "Erlang"], ["actions", "GitHub Actions"], ["pub", "Pub"], ["swift", "Swift"], ["other", "Other"],
];
export const CREDIT_TYPES: [string, string][] = [
  ["finder", "Finder"], ["reporter", "Reporter"], ["analyst", "Analyst"], ["coordinator", "Coordinator"],
  ["remediation_developer", "Remediation developer"], ["remediation_reviewer", "Remediation reviewer"],
  ["remediation_verifier", "Remediation verifier"], ["tool", "Tool"], ["sponsor", "Sponsor"], ["other", "Other"],
];
export const humanize = (s: string | null | undefined) => (s ? (s[0].toUpperCase() + s.slice(1)).replace(/_/g, " ") : "");

export const ecoLabel = (e: string) => ECOSYSTEMS.find(([k]) => k === e)?.[1] || e;
