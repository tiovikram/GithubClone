-- Actions OIDC (area "finish"): the site's ID-token signing key and per-job request tokens.
-- The RSA private key (PKCS#8, base64) is sealed for the X25519 key pair
-- actions_public_keys('oidc', 0), whose private half is itself sealed with the master key.
CREATE TABLE actions_oidc_keys (
    id                 SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    kid                TEXT NOT NULL,
    public_key         BYTEA NOT NULL,
    sealed_private_key TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ACTIONS_ID_TOKEN_REQUEST_TOKEN of a job (sha256), with the job's claims (all but aud/iat/exp).
CREATE TABLE actions_oidc_request_tokens (
    token_hash TEXT PRIMARY KEY,
    job_id     BIGINT NOT NULL REFERENCES actions_jobs(id) ON DELETE CASCADE,
    claims     JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX actions_oidc_request_tokens_job ON actions_oidc_request_tokens (job_id);
