-- Resolved pull request review threads (GraphQL resolveReviewThread); keyed by the thread's
-- top-level review comment.
CREATE TABLE review_thread_resolutions (
    root_comment_id BIGINT PRIMARY KEY REFERENCES review_comments(id) ON DELETE CASCADE,
    resolved_by     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    resolved_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
