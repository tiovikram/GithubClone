import { Route } from "react-router-dom";
import type { Area } from "../types";
import { DependencyGraphPage, DependencyReview } from "./DependencyGraph";
import { GlobalAdvisoriesPage, GlobalAdvisoryPage } from "./GlobalAdvisories";
import { EnterpriseCodeSecurity, OrgCodeSecurity, OrgDependabotSecrets, OrgSecurityOverview } from "./OrgSettings";
import { SecurityLayout } from "./SecurityLayout";
import { ShieldIcon } from "./ui";
import { AdvancedSecuritySettings, DependabotSecretsSettings } from "./Settings";
import "./styles.css";

// The security area: Security tab, dependency graph, dependency review on PRs, advisory
// database, and the security settings sections at repository, organization and enterprise level.
export const area: Area = {
  name: "security",
  topRoutes: (
    <>
      <Route path="advisories" element={<GlobalAdvisoriesPage />} />
      <Route path="advisories/:ghsa" element={<GlobalAdvisoryPage />} />
    </>
  ),
  repoRoutes: (
    <>
      <Route path="security/*" element={<SecurityLayout />} />
      <Route path="network/dependencies" element={<DependencyGraphPage />} />
    </>
  ),
  repoTabs: [{ key: "security", label: "Security", icon: ShieldIcon, path: "security", order: 60 }],
  repoSettings: [
    { key: "security-analysis", path: "security_analysis", label: "Advanced Security", group: "Security", order: 600, element: AdvancedSecuritySettings },
    { key: "secrets-dependabot", path: "secrets/dependabot", label: "Secrets: Dependabot", group: "Security", order: 620, element: DependabotSecretsSettings },
  ],
  orgSettings: [
    { key: "security-products", path: "security_products", label: "Code security", group: "Security", order: 600, element: OrgCodeSecurity, show: (c) => c.isOwner },
    { key: "security-overview", path: "security_overview", label: "Security overview", group: "Security", order: 605, element: OrgSecurityOverview, show: (c) => c.isOwner },
    { key: "secrets-dependabot", path: "secrets/dependabot", label: "Secrets: Dependabot", group: "Security", order: 620, element: OrgDependabotSecrets, show: (c) => c.isOwner },
  ],
  adminSections: [
    { key: "code-security", path: "code-security", label: "Code security", group: "Security", order: 600, element: EnterpriseCodeSecurity },
  ],
  userMenu: [{ key: "advisories", label: "Advisory database", to: "/advisories", order: 600 }],
  slots: { "pull.merge.top": [DependencyReview] },
};
