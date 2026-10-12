-- Pull requests: mergeability cache, test-merge commit, head ref state, review requests,
-- extra review-comment positions.

ALTER TABLE pull_requests
    ADD COLUMN mergeable        BOOLEAN,
    ADD COLUMN rebaseable       BOOLEAN,
    -- "<base_sha>:<head_sha>" the cached mergeable/rebaseable/test_merge_sha were computed for
    ADD COLUMN mergeable_key    TEXT,
    -- sha of refs/pull/N/merge (GitHub reports it as merge_commit_sha while the PR is open)
    ADD COLUMN test_merge_sha   TEXT,
    ADD COLUMN head_ref_deleted BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE pull_review_requests (
    issue_id     BIGINT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    requester_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issue_id, user_id)
);

ALTER TABLE pull_reviews
    ADD COLUMN dismissal_message TEXT;

ALTER TABLE review_comments
    ADD COLUMN original_position   BIGINT,
    ADD COLUMN original_start_line BIGINT,
    ADD COLUMN subject_type        TEXT NOT NULL DEFAULT 'line';

CREATE INDEX review_comments_repo ON review_comments (repo_id, id);
CREATE INDEX review_comments_review ON review_comments (review_id);
CREATE INDEX pull_requests_head_sha ON pull_requests (repo_id, head_sha);
