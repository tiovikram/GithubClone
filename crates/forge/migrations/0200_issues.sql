-- issues area: counters kept in sync by triggers (so every writer -- issues, pulls, GraphQL --
-- stays consistent without coordinating), watch flags, and search/list indexes.

-- repositories.open_issues_count = open issues + open pull requests (GitHub semantics).
CREATE OR REPLACE FUNCTION forge_issue_open_count() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state = 'open' THEN
            UPDATE repositories SET open_issues_count = open_issues_count + 1 WHERE id = NEW.repo_id;
        END IF;
    ELSIF TG_OP = 'DELETE' THEN
        IF OLD.state = 'open' THEN
            UPDATE repositories SET open_issues_count = GREATEST(open_issues_count - 1, 0) WHERE id = OLD.repo_id;
        END IF;
    ELSIF OLD.state IS DISTINCT FROM NEW.state THEN
        IF NEW.state = 'open' THEN
            UPDATE repositories SET open_issues_count = open_issues_count + 1 WHERE id = NEW.repo_id;
        ELSE
            UPDATE repositories SET open_issues_count = GREATEST(open_issues_count - 1, 0) WHERE id = NEW.repo_id;
        END IF;
    END IF;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER issues_open_count AFTER INSERT OR DELETE OR UPDATE OF state ON issues
    FOR EACH ROW EXECUTE FUNCTION forge_issue_open_count();

-- issues.comments_count = number of issue comments (not review comments, like GitHub).
CREATE OR REPLACE FUNCTION forge_issue_comments_count() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE issues SET comments_count = comments_count + 1 WHERE id = NEW.issue_id;
    ELSE
        UPDATE issues SET comments_count = GREATEST(comments_count - 1, 0) WHERE id = OLD.issue_id;
    END IF;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER issue_comments_count AFTER INSERT OR DELETE ON issue_comments
    FOR EACH ROW EXECUTE FUNCTION forge_issue_comments_count();

-- repositories.stargazers_count.
CREATE OR REPLACE FUNCTION forge_stargazers_count() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE repositories SET stargazers_count = stargazers_count + 1 WHERE id = NEW.repo_id;
    ELSE
        UPDATE repositories SET stargazers_count = GREATEST(stargazers_count - 1, 0) WHERE id = OLD.repo_id;
    END IF;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER stars_count AFTER INSERT OR DELETE ON stars
    FOR EACH ROW EXECUTE FUNCTION forge_stargazers_count();

-- Bring existing rows in line (no-op on a fresh database).
UPDATE repositories r SET
    open_issues_count = (SELECT count(*) FROM issues i WHERE i.repo_id = r.id AND i.state = 'open'),
    stargazers_count = (SELECT count(*) FROM stars s WHERE s.repo_id = r.id);
UPDATE issues i SET comments_count = (SELECT count(*) FROM issue_comments c WHERE c.issue_id = i.id);

-- Watching: a row is either a subscription or an explicit "ignore".
ALTER TABLE watches ADD COLUMN subscribed BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE watches ADD COLUMN ignored BOOLEAN NOT NULL DEFAULT FALSE;
CREATE INDEX watches_repo ON watches (repo_id, created_at);
CREATE INDEX stars_repo ON stars (repo_id, created_at);

-- List/search support.
CREATE INDEX issues_repo_updated ON issues (repo_id, updated_at DESC);
CREATE INDEX issues_fts ON issues USING gin (to_tsvector('simple', title || ' ' || coalesce(body, '')));
CREATE INDEX issue_assignees_user ON issue_assignees (user_id);
CREATE INDEX issue_comments_user ON issue_comments (user_id);
CREATE INDEX issue_events_repo ON issue_events (repo_id, id);
CREATE INDEX labels_repo ON labels (repo_id, id);
