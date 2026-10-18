-- Social web UI build-out: profile "hireable", user blocks, custom repository watching.
ALTER TABLE users ADD COLUMN IF NOT EXISTS hireable BOOLEAN;

-- `user_id` blocks `blocked_id` (github.com `/user/blocks`).
CREATE TABLE user_blocks (
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, blocked_id)
);
CREATE INDEX user_blocks_blocked ON user_blocks (blocked_id);

-- "Custom" watching: NULL = all activity; otherwise the subject kinds that notify
-- ('issues', 'pulls', 'releases').
ALTER TABLE watches ADD COLUMN IF NOT EXISTS custom_events TEXT[];
