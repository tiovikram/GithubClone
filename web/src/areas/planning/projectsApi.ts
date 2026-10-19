// Projects (v2) over GraphQL (plus REST for creating views).
import { request } from "../../api";
import { gql } from "./api";

export type OwnerKind = "users" | "orgs";

export interface ProjectSummary {
  id: string;
  number: number;
  title: string;
  shortDescription: string | null;
  closed: boolean;
  public: boolean;
  updatedAt: string;
  items: { totalCount: number };
  owner: { __typename: string; login: string };
}

export interface SelectOptionT { id: string; name: string; color: string; description: string }
export interface Iteration { id: string; title: string; startDate: string; duration: number }
export interface FieldT {
  __typename: "ProjectV2Field" | "ProjectV2SingleSelectField" | "ProjectV2IterationField";
  id: string;
  databaseId: number;
  name: string;
  dataType: string;
  options?: SelectOptionT[];
  configuration?: { duration: number; startDay: number; iterations: Iteration[]; completedIterations: Iteration[] };
}

export interface ViewT {
  id: string;
  number: number;
  name: string;
  layout: "TABLE_LAYOUT" | "BOARD_LAYOUT" | "ROADMAP_LAYOUT";
  filter: string | null;
  verticalGroupByFields: { nodes: { id?: string }[] };
  groupByFields: { nodes: { id?: string }[] };
}

export interface ContentT {
  __typename: "Issue" | "PullRequest" | "DraftIssue";
  id: string;
  title: string;
  body?: string;
  number?: number;
  state?: string;
  merged?: boolean;
  url?: string;
  repository?: { nameWithOwner: string };
  assignees?: { nodes: { login: string }[] };
  labels?: { nodes: { name: string; color: string }[] };
}

export interface FieldValueT {
  __typename: string;
  text?: string;
  number?: number;
  date?: string;
  name?: string;
  optionId?: string;
  title?: string;
  iterationId?: string;
  field?: { id?: string; name?: string };
}

export interface ItemT {
  id: string;
  type: "ISSUE" | "PULL_REQUEST" | "DRAFT_ISSUE" | "REDACTED";
  isArchived: boolean;
  content: ContentT | null;
  fieldValues: { nodes: FieldValueT[] };
}

export interface ProjectT {
  id: string;
  number: number;
  title: string;
  shortDescription: string | null;
  readme: string | null;
  public: boolean;
  closed: boolean;
  viewerCanUpdate: boolean;
  url: string;
  owner: { __typename: string; id: string; login: string };
  views: { nodes: ViewT[] };
  fields: { nodes: FieldT[] };
  items: { totalCount: number; nodes: ItemT[] };
  repositories: { nodes: { id: string; nameWithOwner: string }[] };
}

const FIELD_REF = `... on ProjectV2Field{id name} ... on ProjectV2SingleSelectField{id name} ... on ProjectV2IterationField{id name}`;
const SUMMARY = `id number title shortDescription closed public updatedAt items(first:0){totalCount} owner{__typename ... on User{login} ... on Organization{login}}`;

export const projects = {
  ownerProjects: async (login: string, includeClosed: boolean) => {
    const d = await gql<{ repositoryOwner: { __typename: string; id: string; login: string; viewerCanCreateProjects?: boolean; projectsV2?: { nodes: ProjectSummary[] } } | null }>(
      `query($l:String!){repositoryOwner(login:$l){__typename id login ... on User{viewerCanCreateProjects projectsV2(first:100){nodes{${SUMMARY}}}} ... on Organization{viewerCanCreateProjects projectsV2(first:100){nodes{${SUMMARY}}}}}}`,
      { l: login },
    );
    const o = d.repositoryOwner;
    return o ? { ...o, projects: (o.projectsV2?.nodes || []).filter((p) => includeClosed || !p.closed) } : null;
  },
  repoProjects: async (owner: string, name: string) => {
    const d = await gql<{ repository: { id: string; owner: { id: string; login: string; __typename: string }; viewerCanCreateProjects: boolean; projectsV2: { nodes: ProjectSummary[] } } | null }>(
      `query($o:String!,$r:String!){repository(owner:$o,name:$r,followRenames:true){id viewerCanCreateProjects owner{__typename id login} projectsV2(first:100){nodes{${SUMMARY}}}}}`,
      { o: owner, r: name },
    );
    return d.repository;
  },
  project: async (login: string, number: number, query?: string | null) => {
    const P = `id number title shortDescription readme public closed viewerCanUpdate url
      owner{__typename ... on User{id login} ... on Organization{id login}}
      views(first:50){nodes{id number name layout filter verticalGroupByFields(first:1){nodes{${FIELD_REF}}} groupByFields(first:1){nodes{${FIELD_REF}}}}}
      fields(first:50){nodes{__typename ... on ProjectV2Field{id databaseId name dataType} ... on ProjectV2SingleSelectField{id databaseId name dataType options{id name color description}} ... on ProjectV2IterationField{id databaseId name dataType configuration{duration startDay iterations{id title startDate duration} completedIterations{id title startDate duration}}}}}
      items(first:100, query:$q){totalCount nodes{id type isArchived
        content{__typename ... on Issue{id number title state url repository{nameWithOwner} assignees(first:5){nodes{login}} labels(first:10){nodes{name color}}}
          ... on PullRequest{id number title state merged url repository{nameWithOwner} assignees(first:5){nodes{login}} labels(first:10){nodes{name color}}}
          ... on DraftIssue{id title body}}
        fieldValues(first:30){nodes{__typename
          ... on ProjectV2ItemFieldTextValue{text field{${FIELD_REF}}}
          ... on ProjectV2ItemFieldNumberValue{number field{${FIELD_REF}}}
          ... on ProjectV2ItemFieldDateValue{date field{${FIELD_REF}}}
          ... on ProjectV2ItemFieldSingleSelectValue{name optionId field{${FIELD_REF}}}
          ... on ProjectV2ItemFieldIterationValue{title iterationId field{${FIELD_REF}}}}}}}
      repositories(first:20){nodes{id nameWithOwner}}`;
    const d = await gql<{ repositoryOwner: { projectV2?: ProjectT | null } | null }>(
      `query($l:String!,$n:Int!,$q:String){repositoryOwner(login:$l){... on User{projectV2(number:$n){${P}}} ... on Organization{projectV2(number:$n){${P}}}}}`,
      { l: login, n: number, q: query || null },
    );
    return d.repositoryOwner?.projectV2 || null;
  },
  create: (ownerId: string, title: string, repositoryId?: string) =>
    gql<{ createProjectV2: { projectV2: { id: string; number: number } } }>(
      `mutation($o:ID!,$t:String!,$r:ID){createProjectV2(input:{ownerId:$o,title:$t,repositoryId:$r}){projectV2{id number}}}`,
      { o: ownerId, t: title, r: repositoryId || null },
    ),
  update: (projectId: string, patch: { title?: string; shortDescription?: string; readme?: string; public?: boolean; closed?: boolean }) =>
    gql(`mutation($i:UpdateProjectV2Input!){updateProjectV2(input:$i){projectV2{id}}}`, { i: { projectId, ...patch } }),
  remove: (projectId: string) => gql(`mutation($p:ID!){deleteProjectV2(input:{projectId:$p}){projectV2{id}}}`, { p: projectId }),
  linkRepo: (projectId: string, repositoryId: string, link: boolean) =>
    gql(link ? `mutation($p:ID!,$r:ID!){linkProjectV2ToRepository(input:{projectId:$p,repositoryId:$r}){repository{id}}}`
      : `mutation($p:ID!,$r:ID!){unlinkProjectV2FromRepository(input:{projectId:$p,repositoryId:$r}){repository{id}}}`, { p: projectId, r: repositoryId }),
  addDraft: (projectId: string, title: string, body?: string) =>
    gql<{ addProjectV2DraftIssue: { projectItem: { id: string } } }>(
      `mutation($p:ID!,$t:String!,$b:String){addProjectV2DraftIssue(input:{projectId:$p,title:$t,body:$b}){projectItem{id}}}`, { p: projectId, t: title, b: body || null }),
  updateDraft: (draftIssueId: string, title: string, body: string) =>
    gql(`mutation($d:ID!,$t:String,$b:String){updateProjectV2DraftIssue(input:{draftIssueId:$d,title:$t,body:$b}){draftIssue{id}}}`, { d: draftIssueId, t: title, b: body }),
  addContent: (projectId: string, contentId: string) =>
    gql(`mutation($p:ID!,$c:ID!){addProjectV2ItemById(input:{projectId:$p,contentId:$c}){item{id}}}`, { p: projectId, c: contentId }),
  setValue: (projectId: string, itemId: string, fieldId: string, value: Record<string, unknown>) =>
    gql(`mutation($p:ID!,$i:ID!,$f:ID!,$v:ProjectV2FieldValue!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:$v}){projectV2Item{id}}}`,
      { p: projectId, i: itemId, f: fieldId, v: value }),
  clearValue: (projectId: string, itemId: string, fieldId: string) =>
    gql(`mutation($p:ID!,$i:ID!,$f:ID!){clearProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f}){projectV2Item{id}}}`, { p: projectId, i: itemId, f: fieldId }),
  archive: (projectId: string, itemId: string, archived: boolean) =>
    gql(archived ? `mutation($p:ID!,$i:ID!){archiveProjectV2Item(input:{projectId:$p,itemId:$i}){item{id}}}`
      : `mutation($p:ID!,$i:ID!){unarchiveProjectV2Item(input:{projectId:$p,itemId:$i}){item{id}}}`, { p: projectId, i: itemId }),
  deleteItem: (projectId: string, itemId: string) =>
    gql(`mutation($p:ID!,$i:ID!){deleteProjectV2Item(input:{projectId:$p,itemId:$i}){deletedItemId}}`, { p: projectId, i: itemId }),
  createField: (projectId: string, dataType: string, name: string, extra: Record<string, unknown> = {}) =>
    gql(`mutation($i:CreateProjectV2FieldInput!){createProjectV2Field(input:$i){projectV2Field{... on ProjectV2Field{id} ... on ProjectV2SingleSelectField{id} ... on ProjectV2IterationField{id}}}}`,
      { i: { projectId, dataType, name, ...extra } }),
  deleteField: (fieldId: string) =>
    gql(`mutation($f:ID!){deleteProjectV2Field(input:{fieldId:$f}){projectV2Field{... on ProjectV2Field{id} ... on ProjectV2SingleSelectField{id} ... on ProjectV2IterationField{id}}}}`, { f: fieldId }),
  /** REST: POST /orgs/{org}/projectsV2/{n}/views or /users/{user_id}/projectsV2/{n}/views */
  createView: (kind: OwnerKind, ownerLogin: string, ownerDbId: number, number: number, body: { name: string; layout: string; filter?: string; vertical_group_by?: number[] }) =>
    request<{ number: number }>(kind === "orgs" ? `/orgs/${encodeURIComponent(ownerLogin)}/projectsV2/${number}/views` : `/users/${ownerDbId}/projectsV2/${number}/views`, { method: "POST", body }),
  ownerDbId: async (login: string) => (await request<{ id: number }>(`/users/${encodeURIComponent(login)}`)).id,
};

export const OPTION_COLORS: Record<string, string> = {
  GRAY: "#8c959f", BLUE: "#0969da", GREEN: "#1a7f37", YELLOW: "#bf8700", ORANGE: "#bc4c00", RED: "#cf222e", PINK: "#bf3989", PURPLE: "#8250df",
};

export function projectHref(p: { number: number; owner: { __typename: string; login: string } }): string {
  return `/${p.owner.__typename === "Organization" ? "orgs" : "users"}/${p.owner.login}/projects/${p.number}`;
}
