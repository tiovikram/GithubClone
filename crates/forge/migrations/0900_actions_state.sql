-- GitHub Actions state without a runner (ghcompat): workflow enable/disable, dispatched runs
-- (they stay queued until cancelled), secrets (stored encrypted by the client, never decrypted)
-- and variables. Secrets/variables belong to a repository (optionally an environment) or an org.

CREATE TABLE actions_workflow_states (
    repo_id     BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    workflow_id BIGINT NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('active', 'disabled_manually')),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, workflow_id)
);

CREATE TABLE actions_runs (
    id                  BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id             BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    workflow_id         BIGINT NOT NULL,
    workflow_path       TEXT NOT NULL,
    name                TEXT NOT NULL,
    run_number          BIGINT NOT NULL,
    run_attempt         INT NOT NULL DEFAULT 1,
    event               TEXT NOT NULL,
    status              TEXT NOT NULL,
    conclusion          TEXT,
    head_branch         TEXT,
    head_sha            TEXT NOT NULL,
    head_commit         JSONB,
    inputs              JSONB,
    actor_id            BIGINT REFERENCES users(id) ON DELETE SET NULL,
    triggering_actor_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    run_started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (repo_id, workflow_id, run_number)
);
CREATE INDEX actions_runs_repo ON actions_runs (repo_id, id DESC);

-- Per-owner public key handed to clients for sealed-box encryption (owner_kind 'repo' | 'org').
CREATE TABLE actions_public_keys (
    owner_kind TEXT NOT NULL,
    owner_id   BIGINT NOT NULL,
    key_id     TEXT NOT NULL,
    key        TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_kind, owner_id)
);

CREATE TABLE actions_secrets (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id         BIGINT REFERENCES repositories(id) ON DELETE CASCADE,
    org_id          BIGINT REFERENCES users(id) ON DELETE CASCADE,
    environment     TEXT NOT NULL DEFAULT '',
    name            TEXT NOT NULL,
    encrypted_value TEXT NOT NULL,
    key_id          TEXT NOT NULL,
    visibility      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((repo_id IS NULL) <> (org_id IS NULL))
);
CREATE UNIQUE INDEX actions_secrets_key ON actions_secrets (COALESCE(repo_id, 0), COALESCE(org_id, 0), environment, name);

CREATE TABLE actions_variables (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id     BIGINT REFERENCES repositories(id) ON DELETE CASCADE,
    org_id      BIGINT REFERENCES users(id) ON DELETE CASCADE,
    environment TEXT NOT NULL DEFAULT '',
    name        TEXT NOT NULL,
    value       TEXT NOT NULL,
    visibility  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((repo_id IS NULL) <> (org_id IS NULL))
);
CREATE UNIQUE INDEX actions_variables_key ON actions_variables (COALESCE(repo_id, 0), COALESCE(org_id, 0), environment, name);
