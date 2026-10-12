-- Pending review requests on pull requests (GraphQL requestReviews / PullRequest.reviewRequests).
-- Owned by the graphql area until the pulls area provides its own; keep the shape if merging.
CREATE TABLE IF NOT EXISTS review_requests (
    issue_id     BIGINT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    requester_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issue_id, user_id)
);
