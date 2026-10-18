-- Sub-issues (api.github.com `/repos/{o}/{r}/issues/{n}/sub_issues`): an issue has at most one
-- parent; a parent's children are ordered by `position`.
CREATE TABLE issue_sub_issues (
    parent_id  BIGINT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    child_id   BIGINT NOT NULL UNIQUE REFERENCES issues(id) ON DELETE CASCADE,
    position   DOUBLE PRECISION NOT NULL,
    created_by BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (parent_id, child_id),
    CHECK (parent_id <> child_id)
);
CREATE INDEX issue_sub_issues_parent ON issue_sub_issues (parent_id, position);
