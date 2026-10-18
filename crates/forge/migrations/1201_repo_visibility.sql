-- One definition of "can user U read repository R", used by every repository list and search
-- query (`db::readable_sql`). It mirrors `orgs::perm` / `db::permission`: public repositories;
-- site admins; the owner; direct collaborators (incl. outside collaborators); for organization
-- repositories, org owners, members when the base permission is not `none`, team grants and
-- organization roles with a repository base role (directly or through a team; child teams
-- inherit their parents' grants). p_user NULL = anonymous.
--
-- Created with function-body checks off because the org tables come from a later migration
-- (1250_orgs) on a fresh database; the body is resolved at first use.
SET LOCAL check_function_bodies = off;

CREATE OR REPLACE FUNCTION forge_repo_readable(p_repo BIGINT, p_user BIGINT) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
WITH RECURSIVE
  repo AS (
    SELECT r.id, r.private, r.owner_id, o.kind FROM repositories r JOIN users o ON o.id = r.owner_id WHERE r.id = p_repo
  ),
  -- Teams of the user in the owning org, plus their ancestors.
  mt(team_id) AS (
    SELECT tm.team_id FROM team_members tm JOIN teams t ON t.id = tm.team_id JOIN repo ON t.org_id = repo.owner_id
     WHERE tm.user_id = p_user AND repo.kind = 'Organization'
    UNION
    SELECT t.parent_id FROM mt JOIN teams t ON t.id = mt.team_id WHERE t.parent_id IS NOT NULL
  )
SELECT EXISTS (
  SELECT 1 FROM repo WHERE
     NOT repo.private
     OR (p_user IS NOT NULL AND (
          repo.owner_id = p_user
       OR EXISTS (SELECT 1 FROM users su WHERE su.id = p_user AND su.site_admin)
       OR EXISTS (SELECT 1 FROM repo_collaborators c WHERE c.repo_id = repo.id AND c.user_id = p_user)
       OR (repo.kind = 'Organization' AND (
              EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = repo.owner_id AND m.user_id = p_user
                         AND (m.role = 'admin'
                              OR COALESCE((SELECT st.default_repository_permission FROM org_settings st WHERE st.org_id = repo.owner_id), 'read') <> 'none'))
           OR EXISTS (SELECT 1 FROM team_repos tr JOIN mt ON mt.team_id = tr.team_id WHERE tr.repo_id = repo.id)
           OR EXISTS (
                SELECT 1 FROM org_roles ro
                 WHERE ro.base_role IS NOT NULL AND (ro.org_id IS NULL OR ro.org_id = repo.owner_id)
                   AND (EXISTS (SELECT 1 FROM org_role_users ru WHERE ru.org_id = repo.owner_id AND ru.role_id = ro.id AND ru.user_id = p_user)
                        OR EXISTS (SELECT 1 FROM org_role_teams rt JOIN mt ON mt.team_id = rt.team_id
                                    WHERE rt.org_id = repo.owner_id AND rt.role_id = ro.id)))
          ))
     ))
)
$$;
