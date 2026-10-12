-- reactions has no FK to its polymorphic subject: drop reactions of deleted issues/comments
-- (including the cascade from a deleted repository).
CREATE OR REPLACE FUNCTION forge_reactions_cleanup() RETURNS trigger AS $$
BEGIN
    DELETE FROM reactions WHERE subject_type = TG_ARGV[0] AND subject_id = OLD.id;
    RETURN NULL;
END
$$ LANGUAGE plpgsql;

CREATE TRIGGER issues_reactions_cleanup AFTER DELETE ON issues
    FOR EACH ROW EXECUTE FUNCTION forge_reactions_cleanup('issue');
CREATE TRIGGER issue_comments_reactions_cleanup AFTER DELETE ON issue_comments
    FOR EACH ROW EXECUTE FUNCTION forge_reactions_cleanup('issue_comment');

DELETE FROM reactions x WHERE x.subject_type = 'issue' AND NOT EXISTS(SELECT 1 FROM issues i WHERE i.id = x.subject_id);
DELETE FROM reactions x WHERE x.subject_type = 'issue_comment' AND NOT EXISTS(SELECT 1 FROM issue_comments c WHERE c.id = x.subject_id);
