-- Actions settings (area actions-settings): decryptable secrets, selected repositories for org
-- secrets/variables, permission policies (repo / org / enterprise), runner groups and the
-- repository OIDC subject-claim customization.

-- Secrets become decryptable server-side: each owner's X25519 private key, sealed with the
-- master key (FORGE_SECRET_KEY or the DB-stored `actions-master-key` in signing_secrets).
-- Format: `v1:<env|db>:<base64(nonce(24) || xsalsa20poly1305 ciphertext)>`. Rows created before
-- this migration have no private key; they are rotated (new key_id) on next use.
ALTER TABLE actions_public_keys ADD COLUMN sealed_private_key TEXT;

-- Repositories an org secret / variable with visibility `selected` is shared with.
CREATE TABLE actions_secret_repos (
    secret_id BIGINT NOT NULL REFERENCES actions_secrets(id) ON DELETE CASCADE,
    repo_id   BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    PRIMARY KEY (secret_id, repo_id)
);
CREATE TABLE actions_variable_repos (
    variable_id BIGINT NOT NULL REFERENCES actions_variables(id) ON DELETE CASCADE,
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    PRIMARY KEY (variable_id, repo_id)
);

-- One policy row per owner: owner_kind 'repo' | 'org' | 'enterprise' (owner_id 0: GHES has a
-- single enterprise). NULL means "not set here, inherit from the level above" for the settings
-- that inherit (workflow token permissions, retention); the others have GitHub's defaults.
CREATE TABLE actions_policies (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('repo', 'org', 'enterprise')),
    owner_id   BIGINT NOT NULL,
    -- repo: Actions enabled for the repository
    enabled BOOLEAN NOT NULL DEFAULT true,
    -- org: enabled_repositories, enterprise: enabled_organizations
    enabled_scope TEXT NOT NULL DEFAULT 'all' CHECK (enabled_scope IN ('all', 'none', 'selected')),
    allowed_actions TEXT NOT NULL DEFAULT 'all' CHECK (allowed_actions IN ('all', 'local_only', 'selected')),
    sha_pinning_required BOOLEAN NOT NULL DEFAULT false,
    github_owned_allowed BOOLEAN NOT NULL DEFAULT true,
    verified_allowed BOOLEAN NOT NULL DEFAULT false,
    patterns_allowed TEXT[] NOT NULL DEFAULT '{}',
    default_workflow_permissions TEXT CHECK (default_workflow_permissions IN ('read', 'write')),
    can_approve_pull_request_reviews BOOLEAN,
    retention_days INT,
    approval_policy TEXT NOT NULL DEFAULT 'first_time_contributors',
    fork_pr_run BOOLEAN NOT NULL DEFAULT false,
    fork_pr_write_tokens BOOLEAN NOT NULL DEFAULT false,
    fork_pr_secrets BOOLEAN NOT NULL DEFAULT false,
    fork_pr_require_approval BOOLEAN NOT NULL DEFAULT true,
    -- repo: who outside the repository may use its actions/reusable workflows (private repos)
    access_level TEXT NOT NULL DEFAULT 'none' CHECK (access_level IN ('none', 'user', 'organization', 'enterprise')),
    -- org: which repositories may use self-hosted runners; enterprise: disable for all orgs
    self_hosted_scope TEXT NOT NULL DEFAULT 'all' CHECK (self_hosted_scope IN ('all', 'none', 'selected')),
    self_hosted_disabled BOOLEAN NOT NULL DEFAULT false,
    -- repo: OIDC `sub` claim customization
    oidc_use_default BOOLEAN NOT NULL DEFAULT true,
    oidc_claim_keys TEXT[] NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_kind, owner_id)
);

-- Selected targets of a policy: list 'enabled' (org → repo ids, enterprise → org ids) and
-- 'self_hosted' (org → repo ids allowed to use self-hosted runners).
CREATE TABLE actions_policy_selected (
    owner_kind TEXT NOT NULL,
    owner_id   BIGINT NOT NULL,
    list       TEXT NOT NULL CHECK (list IN ('enabled', 'self_hosted')),
    target_id  BIGINT NOT NULL,
    PRIMARY KEY (owner_kind, owner_id, list, target_id)
);

-- Self-hosted runner groups of an org or of the enterprise (owner_id 0).
CREATE TABLE actions_runner_groups (
    id                         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner_kind                 TEXT NOT NULL CHECK (owner_kind IN ('org', 'enterprise')),
    owner_id                   BIGINT NOT NULL,
    name                       TEXT NOT NULL,
    visibility                 TEXT NOT NULL DEFAULT 'all' CHECK (visibility IN ('all', 'selected', 'private')),
    is_default                 BOOLEAN NOT NULL DEFAULT false,
    allows_public_repositories BOOLEAN NOT NULL DEFAULT false,
    restricted_to_workflows    BOOLEAN NOT NULL DEFAULT false,
    selected_workflows         TEXT[] NOT NULL DEFAULT '{}',
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX actions_runner_groups_name ON actions_runner_groups (owner_kind, owner_id, lower(name));
CREATE UNIQUE INDEX actions_runner_groups_default ON actions_runner_groups (owner_kind, owner_id) WHERE is_default;

-- Repositories (org groups) / organizations (enterprise groups) a `selected` group serves.
CREATE TABLE actions_runner_group_targets (
    group_id  BIGINT NOT NULL REFERENCES actions_runner_groups(id) ON DELETE CASCADE,
    target_id BIGINT NOT NULL,
    PRIMARY KEY (group_id, target_id)
);

-- Runner membership. A runner not listed here belongs to its owner's default group. runner_id
-- refers to the self-hosted runner registry owned by the runner area (no FK across areas).
CREATE TABLE actions_runner_group_runners (
    runner_id BIGINT PRIMARY KEY,
    group_id  BIGINT NOT NULL REFERENCES actions_runner_groups(id) ON DELETE CASCADE
);
CREATE INDEX actions_runner_group_runners_group ON actions_runner_group_runners (group_id);
