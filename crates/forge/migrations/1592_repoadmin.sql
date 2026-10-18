-- Repository traffic (web UI area "repoadmin"): page views reported by the web UI and git clones
-- (upload-pack advertisements), kept for 15 days and aggregated by the /traffic endpoints.
CREATE TABLE repo_traffic (
    id       BIGSERIAL PRIMARY KEY,
    repo_id  BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    kind     TEXT NOT NULL CHECK (kind IN ('view', 'clone')),
    -- "u:<user id>" or "ip:<address>": the unit of `uniques`.
    visitor  TEXT NOT NULL,
    path     TEXT,
    title    TEXT,
    -- Referring host ("google.com"), views only.
    referrer TEXT,
    at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX repo_traffic_repo_kind_at ON repo_traffic (repo_id, kind, at);

-- Collaborators of organization repositories may hold a custom repository role (stored by name
-- and resolved to its base role, like team grants), so the built-in-levels-only check goes.
ALTER TABLE repo_collaborators DROP CONSTRAINT IF EXISTS repo_collaborators_permission_check;

-- Enterprise rulesets (`/enterprises/{e}/rulesets`): neither a repository nor an organization.
ALTER TABLE rulesets ADD COLUMN enterprise BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE rulesets DROP CONSTRAINT IF EXISTS rulesets_check;
ALTER TABLE rulesets ADD CONSTRAINT rulesets_level_check
    CHECK ((enterprise AND repo_id IS NULL AND org_id IS NULL) OR (NOT enterprise AND (repo_id IS NULL) <> (org_id IS NULL)));
CREATE INDEX rulesets_enterprise_idx ON rulesets (id) WHERE enterprise;

-- Pull request options of `PATCH /repos/{o}/{r}` (NULL = GitHub's default), see api::repo_merge_opts.
ALTER TABLE repo_settings
    ADD COLUMN merge_commit_title TEXT,
    ADD COLUMN merge_commit_message TEXT,
    ADD COLUMN squash_merge_commit_title TEXT,
    ADD COLUMN squash_merge_commit_message TEXT,
    ADD COLUMN allow_update_branch BOOLEAN,
    ADD COLUMN use_squash_pr_title_as_default BOOLEAN,
    ADD COLUMN web_commit_signoff_required BOOLEAN;
