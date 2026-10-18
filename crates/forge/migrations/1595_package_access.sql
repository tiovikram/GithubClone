-- Granular package access ("Manage access" on a package's settings page): users granted a role
-- on one package, in addition to what the namespace and the linked repository give them.
CREATE TABLE package_access (
    package_id  BIGINT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('read', 'write', 'admin')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (package_id, user_id)
);
CREATE INDEX package_access_user ON package_access (user_id);
