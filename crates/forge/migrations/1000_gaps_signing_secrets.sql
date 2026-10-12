-- Server-wide secrets shared by all pods (e.g. the key signing archive download URLs).
-- Generated once by the first pod that needs one (INSERT .. ON CONFLICT DO NOTHING, re-read).
CREATE TABLE signing_secrets (
    name       TEXT PRIMARY KEY,
    secret     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
