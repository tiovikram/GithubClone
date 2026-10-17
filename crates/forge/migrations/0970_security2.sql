-- Security follow-ups (docs/security.md).

-- Login throttle shared by every replica (crate::security::login_throttled / record_login).
-- One row per throttle key ("l:<login>|<client>" or "c:<client>"); rows whose window has
-- expired are reset on the next failure and pruned periodically.
CREATE TABLE login_throttle (
    key          TEXT PRIMARY KEY,
    window_start TIMESTAMPTZ NOT NULL DEFAULT now(),
    count        INT NOT NULL DEFAULT 0
);
CREATE INDEX login_throttle_window ON login_throttle (window_start);

-- Free-text issue search (`api/search.rs`): `ILIKE '%word%'` on title/body/comment bodies is
-- served by trigram indexes instead of a scan over every visible issue. pg_trgm is a trusted
-- extension (the database owner can create it).
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX issues_title_trgm ON issues USING gin (title gin_trgm_ops);
CREATE INDEX issues_body_trgm ON issues USING gin (body gin_trgm_ops);
CREATE INDEX issue_comments_body_trgm ON issue_comments USING gin (body gin_trgm_ops);
-- A bare number in the query also matches issue numbers across repositories.
CREATE INDEX issues_number ON issues (number);
