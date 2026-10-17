-- Per-repository check suite preferences (PATCH /repos/{o}/{r}/check-suites/preferences):
-- whether suites are created automatically on push for an app.
CREATE TABLE check_suite_preferences (
    repo_id    BIGINT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    app_id     BIGINT NOT NULL,
    setting    BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_id, app_id)
);
