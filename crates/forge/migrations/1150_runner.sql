-- runner area: GitHub Actions execution. Workflow runs get jobs that `forge runner` processes
-- (claimed with FOR UPDATE SKIP LOCKED), per-attempt history, job logs (chunks in object
-- storage), artifacts, caches, self-hosted runners and their registration tokens, job tokens
-- (GITHUB_TOKEN), schedules and OIDC subject customisation.

ALTER TABLE actions_runs
    ADD COLUMN git_ref        TEXT,
    -- GITHUB_SHA (the merge commit for pull_request runs; head_sha is the PR head)
    ADD COLUMN github_sha     TEXT,
    ADD COLUMN display_title  TEXT,
    ADD COLUMN event_payload  JSONB,
    ADD COLUMN definition     JSONB,
    ADD COLUMN pull_requests  JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN head_repo_id   BIGINT,
    ADD COLUMN completed_at   TIMESTAMPTZ;

CREATE TABLE actions_run_attempts (
    run_id              BIGINT NOT NULL REFERENCES actions_runs(id) ON DELETE CASCADE,
    attempt             INT NOT NULL,
    status              TEXT NOT NULL,
    conclusion          TEXT,
    triggering_actor_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    run_started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, attempt)
);

CREATE TABLE actions_jobs (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id           BIGINT NOT NULL REFERENCES actions_runs(id) ON DELETE CASCADE,
    repo_id          BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    run_attempt      INT NOT NULL,
    job_key          TEXT NOT NULL,
    name             TEXT NOT NULL,
    matrix           JSONB,
    needs            TEXT[] NOT NULL DEFAULT '{}',
    definition       JSONB NOT NULL,
    labels           TEXT[] NOT NULL DEFAULT '{}',
    -- pending (waiting for needs) | queued | in_progress | completed
    status           TEXT NOT NULL,
    conclusion       TEXT,
    steps            JSONB NOT NULL DEFAULT '[]',
    outputs          JSONB NOT NULL DEFAULT '{}',
    runner_id        BIGINT,
    runner_name      TEXT,
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    -- Compacted log (object key), its size, and per-step byte ranges [{number, offset, len}].
    log_key          TEXT,
    log_size         BIGINT,
    step_logs        JSONB,
    heartbeat_at     TIMESTAMPTZ,
    timeout_at       TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at       TIMESTAMPTZ,
    completed_at     TIMESTAMPTZ
);
CREATE INDEX actions_jobs_run ON actions_jobs (run_id, run_attempt, id);
CREATE INDEX actions_jobs_queued ON actions_jobs (id) WHERE status = 'queued';
CREATE INDEX actions_jobs_active ON actions_jobs (heartbeat_at) WHERE status = 'in_progress';

-- Log chunks streamed by the runner while a job runs; objects at actions/logs/{job_id}/{id}.
CREATE TABLE actions_log_chunks (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job_id     BIGINT NOT NULL REFERENCES actions_jobs(id) ON DELETE CASCADE,
    step       INT NOT NULL,
    size       INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX actions_log_chunks_job ON actions_log_chunks (job_id, id);

-- GITHUB_TOKEN: valid for one job, one repository, until the job completes or expires.
CREATE TABLE actions_job_tokens (
    token_hash TEXT PRIMARY KEY,
    job_id     BIGINT NOT NULL REFERENCES actions_jobs(id) ON DELETE CASCADE,
    repo_id    BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    -- read | write (contents permission of the job)
    perm       TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX actions_job_tokens_job ON actions_job_tokens (job_id);

-- Self-hosted runners. repo_id set = repository runner, org_id set = organization runner,
-- neither = enterprise (instance-wide) runner.
CREATE TABLE actions_runners (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id         BIGINT REFERENCES repositories(id) ON DELETE CASCADE,
    org_id          BIGINT REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    os              TEXT NOT NULL DEFAULT 'Linux',
    arch            TEXT NOT NULL DEFAULT 'X64',
    labels          TEXT[] NOT NULL DEFAULT '{}',
    credential_hash TEXT UNIQUE,
    ephemeral       BOOLEAN NOT NULL DEFAULT FALSE,
    version         TEXT,
    runner_group_id BIGINT NOT NULL DEFAULT 1,
    last_seen_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (repo_id IS NULL OR org_id IS NULL)
);
CREATE UNIQUE INDEX actions_runners_name ON actions_runners (COALESCE(repo_id, 0), COALESCE(org_id, 0), lower(name));

-- Registration and remove tokens (GitHub: one hour, reusable until they expire).
CREATE TABLE actions_runner_tokens (
    token_hash TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('registration', 'remove')),
    repo_id    BIGINT REFERENCES repositories(id) ON DELETE CASCADE,
    org_id     BIGINT REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE actions_artifacts (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id       BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    run_id        BIGINT REFERENCES actions_runs(id) ON DELETE CASCADE,
    job_id        BIGINT,
    name          TEXT NOT NULL,
    size_in_bytes BIGINT NOT NULL,
    object_key    TEXT NOT NULL,
    digest        TEXT,
    head_branch   TEXT,
    head_sha      TEXT,
    expired       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ
);
CREATE INDEX actions_artifacts_repo ON actions_artifacts (repo_id, id DESC);
CREATE INDEX actions_artifacts_run ON actions_artifacts (run_id);

CREATE TABLE actions_caches (
    id               BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    repo_id          BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    key              TEXT NOT NULL,
    version          TEXT NOT NULL,
    git_ref          TEXT NOT NULL,
    size_in_bytes    BIGINT NOT NULL,
    object_key       TEXT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (repo_id, git_ref, key, version)
);

-- Cache size policy: scope_kind 'repo' (scope_id = repo id) or 'enterprise' (scope_id = 0).
CREATE TABLE actions_cache_policies (
    scope_kind                      TEXT NOT NULL,
    scope_id                        BIGINT NOT NULL,
    repo_cache_size_limit_in_gb     INT,
    max_repo_cache_size_limit_in_gb INT,
    PRIMARY KEY (scope_kind, scope_id)
);

-- OIDC subject claim customisation: scope_kind 'repo' | 'org'.
CREATE TABLE actions_oidc_subs (
    scope_kind         TEXT NOT NULL,
    scope_id           BIGINT NOT NULL,
    use_default        BOOLEAN NOT NULL DEFAULT TRUE,
    include_claim_keys TEXT[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (scope_kind, scope_id)
);

-- `on: schedule` crons of each repository's default-branch workflows, refreshed on push.
CREATE TABLE actions_schedules (
    repo_id       BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    workflow_path TEXT NOT NULL,
    cron          TEXT NOT NULL,
    last_fired_at TIMESTAMPTZ,
    PRIMARY KEY (repo_id, workflow_path, cron)
);

-- Reviews of deployment environments for a run (get-reviews-for-run).
CREATE TABLE actions_run_approvals (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id       BIGINT NOT NULL REFERENCES actions_runs(id) ON DELETE CASCADE,
    environments JSONB NOT NULL DEFAULT '[]',
    state        TEXT NOT NULL,
    user_id      BIGINT REFERENCES users(id) ON DELETE SET NULL,
    comment      TEXT NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

