# gh CLI ↔ forge protocol specification

*Target client:* stock GitHub CLI **v2.102.0** (`/opt/homebrew/bin/gh`, built with go1.27.1), pointed at forge as a **GitHub Enterprise Server (GHES)** host.
*Sources:* github.com/cli/cli @ `v2.102.0`; github.com/cli/go-gh **v2.16.1** (per cli `go.mod`); github.com/cli/shurcooL-graphql **v0.0.4** (struct→query generator, via go-gh); github.com/shurcooL/githubv4 @ `48295856cce7` (input/enum types); Go `crypto/x509` @ go1.27.1. File paths without prefix are relative to the cli repo root; `go-gh/…`, `shurcooL/…` are relative to those repos.
*Verification:* besides source reading, the request sequences and query texts were checked against the real gh binary talking to a schema-validating mock over `http_unix_socket` (Appendix A/B).

## TL;DR for implementers

1. **http vs https.** gh uses plain `http://` **only** for hostname `github.localhost` (and `*.github.localhost`), which it treats as github.com-like, not GHES. Every other hostname is GHES and always `https://HOST/api/v3/` + `https://HOST/api/graphql`. No env var or config key changes the scheme. On macOS, `SSL_CERT_FILE`/`SSL_CERT_DIR` are ignored (Go uses Security.framework on darwin).
2. **The escape hatch is `http_unix_socket`** in gh's `config.yml`. gh then sends *all* API traffic as **plain HTTP/1.1 over that unix socket, even for `https://` URLs** (go-gh sets `DialTLS` to a raw unix dial). Host header and paths stay the GHES ones (`Host: forge.localhost`, `/api/v3/...`, `/api/graphql`). Verified with the real binary.
3. **Recommended local test setup:** host `forge.localhost` (GHES path, resolves to loopback on macOS) + `GH_CONFIG_DIR=<tmp>` with `http_unix_socket: /abs/forge.sock` + `GH_ENTERPRISE_TOKEN` (or `gh auth login --with-token --insecure-storage`). For git, use a test-only `GIT_CONFIG_GLOBAL` with `url."http://forge.localhost:8080/".insteadOf=https://forge.localhost/` and a static credential helper. This needs no TLS, no sudo and no ports below 1024 (§1.6).
4. **Minimum to get through `gh auth login --with-token`:** `GET /api/v3/` → 200 (optional `X-OAuth-Scopes: repo, read:org`). Then `POST /api/graphql` `query UserCurrent{viewer{login}}`.
5. **GHES feature detection** happens via `__type` introspection and `GET /api/v3/meta` (`installed_version`). The simplest path: `installed_version: "3.20.0"`, and leave the gated fields listed in §5.2 out of the schema.
6. **GraphQL server must be GitHub-lenient** (§4.6). It must allow overlapping fields with different types in disjoint inline fragments (`Issue.state` vs `PullRequest.state`, `User.name` vs `Team.name`), ignore undeclared variables, and treat `ID`/`String` variables as compatible. It must return exactly the selected fields (gh's decoder rejects extra keys), always use HTTP 200, and report missing objects with errors carrying `"type":"NOT_FOUND"`.

## Contents

1. Host handling, URL prefixes, http vs https, TLS, unix sockets
2. Authentication (login, status, token formats, git credential helper)
3. Per-command protocol: 3.0 feature detection; 3.1 repo & label; 3.2 issue; 3.3 pr create/list/view/checkout/diff/status + PR field builder; 3.4 pr merge/close/review; 3.5 release; 3.6 gh api
4. Response headers, GHES version detection, error shapes, GraphQL wire format & required leniency
5. Consolidated GraphQL schema subset (SDL)
6. Consolidated REST endpoint list
- Appendix A: empirical verification and observed request sequences
- Appendix B: verbatim captured GraphQL requests

## 1. Host handling, URL prefixes, http vs https, TLS, unix sockets

All paths below are relative to the cli repo at tag `v2.102.0` unless prefixed `go-gh/` (github.com/cli/go-gh **v2.16.1**, the version pinned in `go.mod:25`) or `shurcooL/` (github.com/cli/shurcooL-graphql **v0.0.4**, used by go-gh for struct-tag queries). gh v2.102.0 is built with **go1.27.1** (`go.mod` `toolchain go1.27.1`; confirmed with `strings /opt/homebrew/bin/gh`).

### 1.1 Host classification

`go-gh/pkg/auth/auth.go:159-198`:

```go
NormalizeHostname(h):  lowercase; "*.github.com" -> "github.com"; "*.github.localhost" -> "github.localhost";
                       "x.y.ghe.com" -> "y.ghe.com"; else unchanged
IsEnterprise(h) = norm != "github.com" && norm != "github.localhost" && !IsTenancy(norm)
IsTenancy(h)    = norm ends with ".ghe.com"
```

So **any** hostname other than `github.com`, `github.localhost` (and their subdomains) and `*.ghe.com` is treated as **GHES**. Note `localhost`, `forge.localhost`, `127.0.0.1`, `example.test` are all GHES. `garage.github.com` is special-cased in `internal/ghinstance/host.go` (normalizes to github.com in go-gh but gets GHES-style URLs in ghinstance).

### 1.2 URL prefixes (`internal/ghinstance/host.go:46-108`, mirrored by `go-gh/pkg/api/rest_client.go:178-190` and `go-gh/pkg/api/graphql_client.go:181-193`)

| Purpose | GHES host `H` | `github.localhost` | github.com |
|---|---|---|---|
| REST base (`RESTPrefix`) | `https://H/api/v3/` | `http://api.github.localhost/` | `https://api.github.com/` |
| GraphQL (`GraphQLEndpoint`) | `https://H/api/graphql` | `http://api.github.localhost/graphql` | `https://api.github.com/graphql` |
| Web/git (`HostPrefix`, used for clone URLs and browse URLs) | `https://H/` | `http://github.localhost/` | `https://github.com/` |
| Gist (`GistPrefix`) | `https://H/gist/` | `http://github.localhost/gist/` | `https://gist.github.com/` |
| User-asset uploads (`UserAssetUploadPrefix`; "GHES does not support this endpoint") | `https://uploads.H/` (unused on GHES) | `http://uploads.github.localhost/` | `https://uploads.github.com/` |
| Release asset uploads | the absolute `upload_url` returned in the release JSON (see release section) — so the server decides; GHES convention is `https://H/api/uploads/repos/{owner}/{repo}/releases/{id}/assets{?name,label}` | same | `https://uploads.github.com/...` |

REST URL composition: `go-gh/pkg/api/rest_client.go:171-176` — if the path starts with `http://` or `https://` it is used verbatim, otherwise `baseURL + path` (gh callers pass paths **without** a leading slash, e.g. `repos/o/r`). `gh api` strips one leading `/` (`pkg/cmd/api/http.go:32`).

Port: `gh auth login --hostname` rejects any hostname containing `:` or `/` (`internal/ghinstance/host.go:36-44`). `GH_HOST=host:8443` is not validated and yields `https://host:8443/api/v3/` (IsEnterprise is true for anything with a port, including `github.localhost:8080`, so a port **disables** the plain-http github.localhost path).

`api_host` (new in this version; `go-gh/pkg/api/host.go`, `go-gh/pkg/api/client_options.go:137-176`, `internal/config/config.go:27,350-380`): a per-host `hosts.yml` key `hosts.<host>.api_host: other.example` swaps only the *host* part of the REST/GraphQL URL (scheme unchanged; must be a bare hostname, no port). Not needed for forge.

### 1.3 Exactly what decides http vs https

The **only** places that emit `http://` are the `strings.EqualFold(hostname, "github.localhost")` branches in `internal/ghinstance/host.go:53-55,66-68,74-76,96-98,103-105` and `go-gh/pkg/api/{rest,graphql}_client.go` (`localhost = "github.localhost"`). Every other hostname gets `https://`. There is **no** env var or config key that switches the scheme. Specifically:

* `GH_HOST`, `GH_ENTERPRISE_TOKEN`/`GITHUB_ENTERPRISE_TOKEN`, `GH_TOKEN`/`GITHUB_TOKEN` only choose host/token (`go-gh/pkg/auth/auth.go:60-100`). For `github.com`, `*.ghe.com` and `github.localhost` the token env vars are `GH_TOKEN`/`GITHUB_TOKEN`; for GHES hosts they are `GH_ENTERPRISE_TOKEN`/`GITHUB_ENTERPRISE_TOKEN`; fallback is `hosts.<host>.oauth_token` in `hosts.yml`, then the keyring.
* `localhost` (bare), `foo.localhost`, `127.0.0.1` → GHES → **https**.
* `github.localhost` and `*.github.localhost` → **http** on port 80, REST at `http://api.github.localhost/` (no `/api/v3` prefix, different `Host:` header), GraphQL at `http://api.github.localhost/graphql`, git/web at `http://github.localhost/`. **Caveat:** `IsEnterprise("github.localhost") == false`, so gh takes the *github.com* code paths (all `featuredetection` answers are "everything supported" without asking, actor-based assignee APIs, `GH_TOKEN` env, telemetry not auto-disabled — set `GH_TELEMETRY=disabled`).

Verified empirically with `/opt/homebrew/bin/gh` 2.102.0: `GH_HOST=github.localhost GH_TOKEN=x gh api user` sent `GET /user` with `Host: api.github.localhost` over plain TCP to port 80 (macOS resolves `*.localhost` to loopback; `dscacheutil -q host -a name api.github.localhost` → 127.0.0.1/::1).

### 1.4 `http_unix_socket` — the escape hatch (plain HTTP even for https:// URLs)

* Config key `http_unix_socket` (global, top-level in `config.yml`; `internal/config/config.go:31,637`). Read by `go-gh/pkg/api/client_options.go:131-133` (`cfg.Get([]string{"http_unix_socket"})`) — gh's `api.NewHTTPClient` (`api/http_client.go:35-89`) passes `Host:"none", AuthToken:"none"` and no transport, so `optionsNeedResolution` is true and the socket is always picked up.
* Transport (`go-gh/pkg/api/http_client.go:237-247`):

  ```go
  dial := func(network, addr string) (net.Conn, error) { return net.Dial("unix", socketPath) }
  return &http.Transport{ Dial: dial, DialTLS: dial, DisableKeepAlives: true }
  ```

  Because `DialTLS` returns a **raw** (non-`*tls.Conn`) connection, Go's `http.Transport` treats it as an already-established TLS connection and speaks **plain HTTP/1.1** over the socket even for `https://` URLs. The URL (and therefore `Host:` header and path, e.g. `Host: forge.localhost`, `/api/v3/...`, `/api/graphql`) is unchanged. Every request also carries `Connection: close` (DisableKeepAlives).
* ALL gh HTTP traffic goes to that socket (all hosts, including any github.com request). It does **not** affect `git` subprocesses.
* Relative socket paths are resolved against gh's cwd; macOS `sun_path` limit is 104 bytes.

Verified empirically: with `http_unix_socket: gh.sock` and `GH_HOST=forge.example GH_ENTERPRISE_TOKEN=tok2`, `gh api user` sent `GET /api/v3/user Host: forge.example` and `gh api graphql` sent `POST /api/graphql` as plain HTTP to a Python `UnixStreamServer`.

### 1.5 TLS trust on macOS (if real HTTPS is used)

* gh does not call `x509.SetFallbackRoots`, has no `InsecureSkipVerify`, and imports no `x509roots/fallback` (grep of cli + go-gh).
* Go 1.27.1 `src/crypto/x509/root_darwin.go:13-14`: `// macOS has no default SSL_CERT_{FILE,DIR} paths. var certFiles, certDirectories []string` and `systemVerify` uses `SecTrustCreateWithCertificates`/`SecTrustEvaluateWithError` (Security.framework). `root_unix.go` (which reads `SSL_CERT_FILE`/`SSL_CERT_DIR`) has build tag `aix || dragonfly || freebsd || (js && wasm) || linux || netbsd || openbsd || solaris || wasip1` — **not darwin**. So **`SSL_CERT_FILE` / `SSL_CERT_DIR` are ignored by gh on macOS.**
* Without sudo, the only way to make gh trust a self-signed CA is to add it to the **user's login keychain** with user-domain trust settings: `security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db ca.pem` (no sudo, but macOS shows a GUI authorization prompt for trust-settings changes, so it is not CI/headless-friendly). Not recommended for automated tests.
* Unprivileged ports: on macOS an unprivileged process may bind ports <1024 only on the wildcard address (`0.0.0.0:80` succeeded; `127.0.0.1:80` → `EACCES`, tested). Binding 0.0.0.0 exposes the server on the LAN.

### 1.6 Recommended local test setups

**A. Recommended (GHES code path, no TLS, no root, no low ports): hostname `forge.localhost` + `http_unix_socket`.**

```sh
export GH_CONFIG_DIR=$PWD/.gh-test           # isolated gh config; never touches ~/.config/gh
export GH_TELEMETRY=disabled GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
printf 'http_unix_socket: /abs/path/forge.sock\n' > $GH_CONFIG_DIR/config.yml
echo "$TOKEN" | gh auth login --hostname forge.localhost --with-token --insecure-storage
# or: export GH_HOST=forge.localhost GH_ENTERPRISE_TOKEN=$TOKEN
```

* forge serves the API as plain HTTP on the unix socket, routing on path: `/api/v3/*` (REST) and `/api/graphql`; it should also serve `/api/uploads/*` there and emit `upload_url`s under `https://forge.localhost/api/uploads/...` (gh will send them over the socket too).
* `forge.localhost` is classified GHES (`IsEnterprise` true) — this is the exact code path real users with a GHES host get, which is our success metric.
* git does not use the socket. gh will tell git to use `https://forge.localhost/OWNER/REPO.git`. In a test-only git config (`GIT_CONFIG_GLOBAL=$PWD/.gitconfig-test`) rewrite to forge's plain-HTTP git listener on a high port:

  ```ini
  [url "http://forge.localhost:8080/"]
      insteadOf = https://forge.localhost/
  [credential "http://forge.localhost:8080"]
      helper = "!f() { echo username=x-access-token; echo password=$FORGE_TOKEN; }; f"
  ```

  This is safe for gh's remote parsing: `git remote -v` then prints `http://forge.localhost:8080/o/r.git`, and gh's `ghrepo.FromURL` (`internal/ghrepo/repo.go:61-72`) uses `u.Hostname()` (port stripped, scheme ignored) → host `forge.localhost`, which matches the authenticated host. (Verified end-to-end: with this insteadOf in effect, `gh pr list` / `gh repo view` with no `-R` inside the checkout resolved `forge.localhost/o/r` from the rewritten remote.) `gh auth git-credential` cannot be used here: it only answers `protocol=https` and looks the token up by `host` including any port (`pkg/cmd/auth/gitcredential/helper.go`), hence the static helper above (or let forge accept the token as Basic-auth password with any username).

**B. Simplest possible (github.com code path): hostname `github.localhost`, plain HTTP on port 80.** `GH_HOST=github.localhost GH_TOKEN=...`; forge listens on `0.0.0.0:80` (or, to avoid binding 80, use `http_unix_socket` for the API plus the same insteadOf trick for git: `http://github.localhost/` → `http://github.localhost:8080/`). Server must route by `Host:` header: `api.github.localhost` → REST at root (`/user`, `/repos/...`) and `/graphql`; `github.localhost` → git smart-HTTP and web. Downside: exercises github.com-only paths (actor APIs: `suggestedActors`, `replaceActorsForAssignable`, `requestReviewsByLogin`, etc.), not the GHES ones; also `gh auth git-credential` refuses non-https so git creds need the static helper anyway.

**C. Real HTTPS on GHES path** (`forge.localhost` on `0.0.0.0:443` with a CA added to the login keychain) — only needed to test the TLS stack itself.

Use `GH_DEBUG=api` to make gh log full request/response headers and bodies to stderr (`go-gh/pkg/api/http_client.go:88-117`, `api/http_client.go:45-54`).

## 2. Authentication

### 2.1 Request headers gh sends on every API call

Built in `api/http_client.go:35-89` + `go-gh/pkg/api/http_client.go:166-235` + `api/client.go:106-164,413-428`. Captured verbatim from gh 2.102.0 (`gh auth login` against forge.localhost over the unix socket):

```
GET /api/v3/ HTTP/1.1
Host: forge.localhost
User-Agent: GitHub CLI 2.102.0                      (optionally " Agent/<name>" when invoked by an AI agent)
Accept: application/vnd.github.merge-info-preview+json, application/vnd.github.nebula-preview
Authorization: token <TOKEN>                        (always the literal "token " scheme, never "Bearer")
Content-Type: application/json; charset=utf-8       (sent even on GET)
Time-Zone: America/Chicago                          (IANA tz of the client)
X-Github-Api-Version: 2022-11-28
Accept-Encoding: gzip
Connection: close                                   (only with http_unix_socket)

POST /api/graphql  — same headers plus:
GraphQL-Features: merge_queue                       (api/client.go:23-24,111,124,136,148)
Content-Type: application/json                      (struct-tag queries via shurcooL override to plain application/json)
```

* `gh api` overrides `Accept: */*` unless `-H Accept:` is given (captured).
* Some calls add `Accept: application/vnd.github.v3.diff|patch|raw` etc. (see per-command sections).
* `X-GH-CACHE-TTL` / `X-GH-CACHE-DIR` are client-internal cache hints (`api/client.go:22`); a server should ignore them if they appear.
* Auth header attachment (`api/http_client.go:153-190`): set for any request whose hostname (port stripped, normalized) has a token; dropped on a cross-host redirect.
* Token sources (`go-gh/pkg/auth/auth.go:60-100`): for GHES hosts `GH_ENTERPRISE_TOKEN` > `GITHUB_ENTERPRISE_TOKEN` > `hosts.yml` `hosts.<host>.oauth_token` > keyring (`gh auth token --secure-storage`). For github.localhost: `GH_TOKEN` > `GITHUB_TOKEN` > config > keyring.
* **Token format: any opaque string is accepted** (`pkg/cmd/auth/login/login.go:118-125` just `strings.TrimSpace`s stdin). Only cosmetic behaviour depends on prefix: `gh auth status` prints/validates scopes only for tokens starting `ghp_` or `gho_` (`pkg/cmd/auth/status/status.go:359-364`). Recommendation: issue tokens with a `ghp_` prefix (and return `X-OAuth-Scopes`) so `gh auth status` shows scopes like GitHub.

### 2.2 `gh auth login --hostname H --with-token`

`pkg/cmd/auth/login/login.go:168-218`. Hostname is lower-cased and must not contain `/` or `:`. If `GH_ENTERPRISE_TOKEN` (or the relevant env var) is set, login refuses ("The value of the ... environment variable is being used").

1. **Scope check** — `shared.HasMinimumScopes` → `GetScopes` (`pkg/cmd/auth/shared/oauth_scopes.go:30-104`):
   `GET https://H/api/v3/` (REST root, **trailing slash, empty path**), header `Authorization: token <T>`.
   * Must return **HTTP 200 exactly** (`res.StatusCode != 200` → error "unexpected HTTP ..."; non-2xx → HTTPError). Body is discarded.
   * Reads response header `X-Oauth-Scopes`. If **absent/empty → no check at all** (treated as integration token). If present it must contain `repo` AND one of `read:org` / `write:org` / `admin:org`, else login fails with `error validating token: missing required scope(s) 'repo', 'read:org'`.
   * Recommended: `X-OAuth-Scopes: repo, read:org, workflow, gist` (or omit the header entirely).
2. **Viewer query** — `shared.GetCurrentLogin` (`pkg/cmd/auth/shared/login_flow.go:263-275`):
   `POST https://H/api/graphql` body exactly
   `{"query":"query UserCurrent{viewer{login}}","variables":null}` (note `"variables":null`).
   Reads `data.viewer.login`.
3. Writes `hosts.yml` (`hosts.H.users.<login>.oauth_token`, `hosts.H.user`, `hosts.H.oauth_token` when `--insecure-storage`; otherwise token goes to the OS keyring). `git_protocol` is only written if `--git-protocol` is given. No git config changes with `--with-token`.

(Interactive/web login uses the OAuth device flow implemented in the external module github.com/cli/oauth — out of scope; `--with-token` is sufficient for gh to work.)

When the token comes from `GH_ENTERPRISE_TOKEN` and `hosts.yml` also has an account for the same host, `gh auth status` checks both (captured: `POST /api/graphql` UserCurrent then `GET /api/v3/` twice).

### 2.3 `gh auth status`

`pkg/cmd/auth/status/status.go:375-424`. For each known host/account:

* If the token came from an env var (`*_TOKEN` source) the login is not stored, so it first runs `api.CurrentLoginName` (`api/queries_user.go:7-15`): `POST /api/graphql` `{"query":"query UserCurrent{viewer{login}}"}` (shurcooL form; no `variables` key).
* Always: `GET /api/v3/` with `Authorization: token <T>` → needs 200; reads `X-Oauth-Scopes` (display + missing-scope warning only for `ghp_`/`gho_` tokens).
* Any error → "X Failed to log in to H ..." and exit status 1.

### 2.4 Git credential helper

`gh auth setup-git [--hostname H]` (`pkg/cmd/auth/setupgit/setupgit.go`, `pkg/cmd/auth/shared/gitcredentials/helper_config.go:22-117`) runs:

```
git config --global --replace-all credential.https://H.helper ""
git config --global --add         credential.https://H.helper "!/opt/homebrew/bin/gh auth git-credential"
```

(key = `credential.` + `HostPrefix(H)` without trailing slash, i.e. `credential.https://H.helper`; for github.localhost `credential.http://github.localhost.helper`. A gist key is added only when the gist host starts with `gist.`, i.e. not for GHES.)

`gh auth git-credential get` (`pkg/cmd/auth/gitcredential/helper.go`): reads git's `key=value` lines (or `url=`), then:
* **Only `protocol=https` is served** — anything else → silent failure (so plain-http git never gets a token from gh, including for github.localhost).
* Looks up token with `cfg.ActiveToken(host)` where `host` is exactly what git sent (**includes `:port`** if any → no match for `H:8443`). Falls back to stripping a `gist.` prefix.
* Username: `x-access-token` if the token came from an env var (`*_TOKEN`), otherwise the stored login for the host (fallback `x-access-token`). If git supplied a username different from the stored login → silent failure.
* Output:
  ```
  protocol=https
  host=H
  username=<login or x-access-token>
  password=<token>
  ```
* `store` / `erase` are accepted no-ops.

**Server implication:** git smart-HTTP must accept HTTP Basic auth where the password is the API token and the username is either the user's login or `x-access-token` (accept any username). Git first tries anonymously and needs a `401` with `WWW-Authenticate: Basic realm="..."` to trigger the credential helper.

## 3. Per-command protocol

### 3.0 Feature detection (`internal/featuredetection/feature_detection.go`)

Exact request bodies below were captured from gh 2.102.0 on the wire (GHES host `forge.localhost`), except `ProjectV2_fields`, which is reconstructed from the struct tags (not exercised).

| Detector | When it queries | Exact request(s) | What gh looks for | Answer giving the simplest/most-compatible path |
|---|---|---|---|---|
| `IssueFeatures()` | GHES only (github.localhost/github.com: returns `ApiActorsSupported=true, IssueRelationshipsSupported=true` with no request) | `{"query":"query Issue_fields{Issue: __type(name: \"Issue\"){fields(includeDeprecated: true){name}}}"}` | field `blockedBy` ⇒ `IssueRelationshipsSupported` (GHES always gets `ApiActorsSupported=false`) | omit `blockedBy` |
| `PullRequestFeatures()` | **Always**, all hosts (short-circuit is commented out, cli/cli#5778) — two parallel queries | `{"query":"query PullRequest_fields{PullRequest: __type(name: \"PullRequest\"){fields(includeDeprecated: true){name}},StatusCheckRollupContextConnection: __type(name: \"StatusCheckRollupContextConnection\"){fields(includeDeprecated: true){name}}}"}` and `{"query":"query PullRequest_fields2{WorkflowRun: __type(name: \"WorkflowRun\"){fields(includeDeprecated: true){name}}}"}` | `PullRequest.isInMergeQueue` ⇒ `MergeQueue`; `StatusCheckRollupContextConnection.checkRunCount` ⇒ `CheckRunAndStatusContextCounts`; `WorkflowRun.event` ⇒ `CheckRunEvent` | omit all three (gh then never asks for `isInMergeQueue`/`isMergeQueueEnabled`, `checkRunCount*`/`statusContextCount*`, or `checkSuite.workflowRun.event`). |
| `RepositoryFeatures()` | GHES only (else all true) | `{"query":"query Repository_fields{Repository: __type(name: \"Repository\"){fields(includeDeprecated: true){name}}}"}` | `pullRequestTemplates` ⇒ `PullRequestTemplateQuery`; `visibility` ⇒ `VisibilityField`; `autoMergeAllowed` ⇒ `AutoMerge` | Include `visibility` (needed for `repo list/view` visibility output); omitting `pullRequestTemplates` makes `pr create` fall back to reading templates from the local git checkout; omitting `autoMergeAllowed` is simplest. |
| `ProjectsV1()` | GHES only | `GET /api/v3/meta` | `installed_version < 3.17.0` ⇒ classic projects supported | `installed_version >= 3.17.0` ⇒ no `projectCards` queries |
| `ProjectFeatures()` | GHES only | `{"query":"query ProjectV2_fields{ProjectV2: __type(name: \"ProjectV2\"){fields(includeDeprecated: true){name,args{name}}}}"}` | `ProjectV2.items(query:)` arg | n/a unless projects used |
| `SearchFeatures()` | GHES: `GET /api/v3/meta` first (**error is fatal**: captured `gh issue list --search foo` → "malformed version: " when `installed_version` missing); then if version ≥ 3.18.0 (or always on non-GHES) the enum query | `{"query":"query SearchType_enumValues{SearchType: __type(name: \"SearchType\"){enumValues(includeDeprecated: true){name}}}"}` | `ISSUE_ADVANCED` ⇒ must opt in with `type: ISSUE_ADVANCED`; `ISSUE_SEMANTIC`, `ISSUE_HYBRID` | Return `installed_version` ≥ 3.18.0 and `SearchType` values `ISSUE, REPOSITORY, USER, DISCUSSION` (no `ISSUE_ADVANCED`) ⇒ gh uses `search(type: ISSUE, query: ...)` with advanced syntax. (Alternatively `installed_version` 3.17.x ⇒ no enum query, legacy search syntax.) |
| `ReleaseFeatures()` | All hosts | `{"query":"query Release_fields{Release: __type(name: \"Release\"){fields{name}}}"}` (note: no `includeDeprecated`) | `immutable` ⇒ `ImmutableReleases` | omit `immutable` |
| `ActionsFeatures()` | GHES only | `GET /api/v3/meta` | ≥ 3.21.0 | n/a for our commands |

Note `variables` key is omitted entirely in these requests (shurcooL, no variables).

## 3.1 Repository & label commands

Conventions used in this section:

- `REST:` paths are relative to the REST base (GHES: `https://HOST/api/v3/`; github.localhost: `http://api.github.localhost/`). `GQL:` means `POST` to the GraphQL endpoint (GHES: `https://HOST/api/graphql`; github.localhost: `http://api.github.localhost/graphql`) with JSON body `{"query": ..., "variables": ...}`.
- Path segments are percent-encoded per segment (`internal/safeurl/safeurl.go:54-103`); a segment that is exactly `..` is rejected client-side.
- REST bodies built with `json.NewEncoder(...).Encode` end with a trailing `\n`. Bodies built with `json.Marshal(map[string]string)` have their keys sorted alphabetically.
- Raw-string GraphQL calls (`api.Client.GraphQL`, `api/client.go:108`) send the query text verbatim, with `"variables": null` when the Go map is nil (`go-gh/pkg/api/graphql_client.go:69`). Every GraphQL request carries `GraphQL-Features: merge_queue` (`api/client.go:111,124,136`).
- Struct-derived GraphQL calls (`api.Client.Query`, which uses `cli/shurcooL-graphql v0.0.4`) produce compact text. Field names are the Go field names with the first letter lowercased and acronyms lowercased (`ID`→`id`, `URL`→`url`, `DatabaseID`→`databaseId`), unless a `graphql:"..."` tag overrides them. Sibling selections are comma-separated with no whitespace. Variables are listed sorted by name with no separator, for example `query N($name:String!$owner:String!){...}`, and are omitted entirely (no parentheses) when there are none. With no variables, the JSON body omits `variables`.
- Any non-empty `errors` array in a GraphQL response is treated as a failure (`go-gh/pkg/api/graphql_client.go:104-106`). The one exception is the `RepositoryNetwork` query, which tolerates errors whose `type` is `"NOT_FOUND"` (`api/queries_repo.go:518-530`).
- REST success means any 2xx. Non-2xx responses with a JSON content type are parsed as `{"message": str, "errors": [str | {resource, field, code, message}]}` (`go-gh/pkg/api/errors.go:103-156`).
- Scope hints: on a 4xx other than 422, if the response carries `X-Oauth-Scopes`, gh compares it with `X-Accepted-Oauth-Scopes` to print an "add scope" hint (`api/client.go:361-`).

### Shared helper requests (referenced below)

**H1. `UserCurrent` (login)**, `api/queries_user.go:7-15` (struct-derived)
```graphql
query UserCurrent{viewer{login}}
```
Reads `data.viewer.login`.

**H2. `UserCurrent` (login + orgs)**, `api/queries_user.go:17-35`
```graphql
query UserCurrent{viewer{login,organizations(first: 100){nodes{login}}}}
```
Reads `viewer.login` and `viewer.organizations.nodes[].login`. There is no pagination; at most 100 orgs are used.

**H3. `UserCurrent` (node id)**, `api/queries_user.go:37-45`
```graphql
query UserCurrent{viewer{id}}
```

**H4. `RepositoryInfo` via `api.GitHubRepo`**, `api/queries_repo.go:360-413` (raw text, variables `{"owner": "...", "name": "..."}`)
```graphql
	fragment repo on Repository {
		id
		databaseId
		name
		owner { login }
		sshUrl
		hasIssuesEnabled
		description
		hasWikiEnabled
		viewerPermission
		defaultBranchRef {
			name
		}
	}

	query RepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			...repo
			parent {
				...repo
			}
			mergeCommitAllowed
			rebaseMergeAllowed
			squashMergeAllowed
		}
	}
```
If `data.repository` is `null` with no `errors`, gh synthesizes a NOT_FOUND error: "Could not resolve to a Repository with the name 'O/R'." (`api/queries_repo.go:401-410`).

**H5. `RepositoryInfo` via `api.FetchRepository(fields)`**, `api/queries_repo.go:286-316`
```graphql
query RepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {<RepositoryGraphQL(fields)>}
	}
```
`RepositoryGraphQL` (`api/query_builder.go:555-610`) joins the per-field selections with `,`. The mapping table is in `gh repo view` below. A `null` repository is handled the same way as in H4.

**H6. Base-repo resolution (`SmartBaseRepoFunc`)**: `pkg/cmd/factory/default.go:153-177`, `context/context.go:61-109`. This applies to every `repo` and `label` subcommand (they are registered on `repoResolvingCmdFactory`, `pkg/cmd/root/root.go:163-176`) whenever they need the "current" repository.

1. Run `git remote -v` and `git config --get-regexp '^remote\..*\.gh-resolved$'`. No HTTP is involved. Remotes are kept only for authenticated hosts, sorted `upstream` > `github` > `origin` > others (`pkg/cmd/factory/remote_resolver.go:28-95`).
2. `-R/--repo` or `GH_REPO` (label commands only, via `cmdutil.EnableRepoOverride`, `pkg/cmd/label/label.go:14`) short-circuits with no HTTP.
3. A remote with `gh-resolved` = `base` or `OWNER/REPO` wins, with no HTTP.
4. If the session cannot prompt (non-TTY), gh takes the first remote, with no HTTP.
5. Otherwise gh sends `RepositoryNetwork` for up to 5 remotes. The request goes to the host of the first remote (`api/queries_repo.go:477-571`). Owner and name are inlined as Go `%q` string literals:
```graphql
	fragment repo on Repository {
		id
		databaseId
		name
		owner { login }
		viewerPermission
		defaultBranchRef {
			name
		}
		isPrivate
	}
	query RepositoryNetwork {
		viewer { login }
		
		repo_000: repository(owner: "OWNER", name: "REPO") {
			...repo
			parent {
				...repo
			}
		}
		
	}
```
`variables` is `null`. Response keys are `viewer` and `repo_NNN`, sorted lexicographically. A null `repo_NNN` is allowed. Any other top-level key is an error ("unknown GraphQL result key"). Unique repos plus their parents are collected: 0 results falls back to the first remote, 1 result is used, and 2 or more produce the error "please run `gh repo set-default`" (`context/context.go:88-108`).

The plain `BaseRepoFunc`, which just takes the first remote, is not used by these commands.

---

### `gh repo create`

Source: `pkg/cmd/repo/create/create.go`, `pkg/cmd/repo/create/http.go`.

**Feature detection / IsEnterprise:** none. GHES and github.localhost follow identical paths, so neither is simpler; only the URL bases differ.

Flag validation (`create.go:121-187`):
- Non-interactive mode, entered when there is any arg or any flag, requires exactly one of `--public`, `--private`, or `--internal`, which sets `Visibility` to `PUBLIC`, `PRIVATE`, or `INTERNAL`.
- `--source` cannot be combined with `--clone`, `--template`, `--license`, or `--gitignore`.
- `--push` and `--remote` require `--source`.

`NAME` may be `REPO`, `OWNER/REPO` or `HOST/OWNER/REPO`. If no owner is given, `OwnerLogin` is `""` (`create.go:342-350`).

#### Core request selection: `repoCreate` (`http.go:77-251`)

Step 1, owner resolution (only when needed):
- `--team SLUG`: **`GET orgs/{owner}/teams/{slug}`** (`http.go:280-288`). Reads `id` (uint64), `node_id`, and `organization.node_id`. Sets `isOrg=true`.
- Otherwise, if an owner was given: **`GET users/{owner}`** (`http.go:262-270`). Reads `node_id` and `type`; `isOrg = (type == "Organization")`. This request is sent even when the owner is the viewer's own login, in which case `type` is `"User"`.
- No owner: no request. `ownerId` is omitted and the server must default it to the viewer.
- `--internal` with `!isOrg` fails client-side with "internal repositories can only be created within an organization" (`http.go:103-106`).

Step 2a, template (`--template`): `cloneTemplateRepository` mutation. See the template path below.

Step 2b, REST path, used if `--gitignore`, `--license` or `--add-readme` is set (`http.go:176-213`):
- **`POST user/repos`** for a personal repo, or **`POST orgs/{owner}/repos`** when `isOrg`.
- Body (`createRepositoryInputV3`, `http.go:33-45`), JSON-encoded with a trailing newline:
  ```json
  {"name":"R","homepage":"H?","description":"D?","private":true|false,"visibility":"public|private|internal"?,"team_id":123?,"has_issues":true,"has_wiki":true,"gitignore_template":"Go"?,"license_template":"mit"?,"auto_init":true?}
  ```
  - `private` is always present and is `true` only when visibility is `PRIVATE`.
  - `visibility` (lowercase) is sent only on the org path; the personal path sends only `private`, so `--internal` cannot reach it.
  - `homepage`, `description`, `team_id`, `gitignore_template`, `license_template` and `auto_init` are omitted when empty, zero or false. `has_issues` and `has_wiki` are always present (true unless `--disable-issues` or `--disable-wiki`).
- Response read (`api/queries_repo.go:582-592,1619-1638`): `node_id`, `name`, `owner.login`, `html_url`, `private`, `created_at`. Expected 201, though any 2xx works.

Step 2c, GraphQL path (the default) (`http.go:215-250`):
```graphql
	mutation RepositoryCreate($input: CreateRepositoryInput!) {
		createRepository(input: $input) {
			repository {
				id
				name
				owner { login }
				url
			}
		}
	}
```
Variables:
```json
{"input":{"name":"R","homepageUrl":"H"?,"description":"D"?,"visibility":"PUBLIC|PRIVATE|INTERNAL","ownerId":"<node id>"?,"teamId":"<team node id>"?,"hasIssuesEnabled":true,"hasWikiEnabled":true}}
```
`homepageUrl`, `description`, `ownerId` and `teamId` are omitted when empty. `visibility`, `hasIssuesEnabled` and `hasWikiEnabled` are always present (`http.go:48-57`). The response fields read are `createRepository.repository.{id,name,owner.login,url}`.

Output: on a TTY, `✓ Created repository OWNER/NAME on HOST\n  URL`; otherwise just `URL` (`create.go:399-410`). `OWNER/NAME` comes from the response `owner.login`/`name`.

#### Non-interactive "from scratch" (`createFromScratch`, `create.go:296-436`), in order

1. With `--template T` (`create.go:368-392`):
   - If `T` has no slash, send H1 to get the login and prefix it.
   - Send H4 for the template repo. Reads `id` and `defaultBranchRef.name`.
2. `repoCreate`, as above.
   - **Template path** (`http.go:108-174`): if there is no `ownerId` yet, send H3 (`viewer{id}`). Then:
     ```graphql
     		mutation CloneTemplateRepository($input: CloneTemplateRepositoryInput!) {
     			cloneTemplateRepository(input: $input) {
     				repository {
     					id
     					name
     					owner { login }
     					url
     				}
     			}
     		}
     ```
     Variables: `{"input":{"name":"R","visibility":"PUBLIC|PRIVATE|INTERNAL","description":"D"?,"ownerId":"...","repositoryId":"<template node id>","includeAllBranches":false}}`. `ownerId`, `repositoryId` and `includeAllBranches` are always present.
     Then, if `--disable-wiki`, `--disable-issues` or `--homepage` is set (`http.go:150-171`):
     ```graphql
     				mutation UpdateRepository($input: UpdateRepositoryInput!) {
     					updateRepository(input: $input) {
     						repository {
     							id
     						}
     					}
     				}
     ```
     Variables: `{"input":{"repositoryId":"<new id>","hasWikiEnabled":bool,"hasIssuesEnabled":bool,"homepageUrl":"H"?}}`.
3. `--clone` (`create.go:420-433`):
   - The protocol comes from config `git_protocol` (default `https`).
   - The URL is `ghrepo.FormatRemoteURL(repo, protocol)` (`internal/ghrepo/repo.go:95-103`), built from the owner and name in the response:
     - `https` on GHES: `https://HOST/OWNER/NAME.git`
     - `https` on github.localhost: `http://github.localhost/OWNER/NAME.git` (`internal/ghinstance/host.go:102-107`)
     - `ssh`: `git@HOST:OWNER/NAME.git`. On `*.ghe.com` the user is the tenant, giving `TENANT@HOST:...`.
   - With no README, license, gitignore or template (`localInit`, `create.go:804-829`), there is no network traffic: `git init NAME`, then `git -C NAME remote add origin URL`.
   - Otherwise (`cloneWithRetry`, `create.go:710-735`):
     ```
     git -c credential.https://HOST.helper= -c credential.https://HOST.helper=!"gh" auth git-credential clone [--branch <templateDefaultBranch>] URL
     ```
     For github.localhost the credential pattern is `http://github.localhost`. On git exit code 128, gh retries up to 3 more times with a constant 3s back-off; this is the only "readiness" wait.

Note: `gh auth git-credential` only answers `protocol=https` (`pkg/cmd/auth/gitcredential/helper.go:103-105`). For github.localhost over plain `http`, gh provides no git credentials, so the git smart-HTTP server must allow the operation without auth or the user must have another credential helper.

#### Interactive (`gh repo create` with no args or flags; TTY required) (`create.go:264-293`)

The first prompt selects one of three flows.

From scratch:
1. `interactiveRepoInfo`: if the typed name has no `OWNER/`, send H2 (login and orgs). An owner is prompted only if the user belongs to at least one org. Choosing your own login leaves owner `""`. "Internal" is offered only when an org owner is chosen (`create.go:895-968`).
2. "Add a .gitignore?": if yes, **`GET gitignore/templates`**, which returns a JSON array of strings (`api/queries_repo.go:1737-1749`).
3. "Add a license?": if yes, **`GET licenses`**, which returns an array of objects; gh reads `key` and `name` (`api/queries_repo.go:1705-1717`). The selected `key` is sent as `license_template`.
4. Confirm, then `repoCreate` as above.
5. "Clone the new repository locally?" defaults to yes.

From template (`create.go:439-531`):
1. Name, description and visibility as above. If the name has no owner, send H2 and prefix the login.
2. List templates with `listTemplateRepositories` (`http.go:290-355`), variables `{"perPage":100,"owner":"OWNER"}` and later `endCursor`:
   ```graphql
   query RepositoryList($perPage:Int!,$endCursor:String,$owner:String!) {
   		repositoryOwner(login: $owner) {
   			login
   			repositories(first: $perPage, after: $endCursor, ownerAffiliations: OWNER, orderBy: { field: PUSHED_AT, direction: DESC }) {
   				nodes{
   					id
   					name
   					isTemplate
   					defaultBranchRef {
   						name
   					}
   				}
   				totalCount
   				pageInfo{hasNextPage,endCursor}
   			}
   		}
   	}
   ```
   gh pages through all results while `hasNextPage` is true and filters `isTemplate == true` client-side.
3. `repoCreate` (template path), then always prompts to clone. The clone uses `--branch <defaultBranchRef.name>`.

Push existing local repo: same as `--source` below, plus prompts.

#### `gh repo create NAME --source . [--push] [--remote R] --public|--private|--internal` (`createFromLocal`, `create.go:534-708`)

1. Local git commands, with no HTTP:
   - `git -C <src> rev-parse --git-dir`: `.git` means a working repo and `.` means bare.
   - `git -C <src> rev-parse HEAD`: exit 128 means no commits; `--push` then fails.
2. The repo name defaults to `basename(abs(src))`. Then `repoCreate`:
   - Owner resolution as above.
   - The GraphQL `RepositoryCreate` path is always taken, because `--source` cannot be combined with license or gitignore and `InitReadme` is never set here.
3. `git -C <src> remote add <remote|origin> URL`, where URL is built as described for `--clone`.
4. `--push` on a working repo:
   ```
   git -C <src> -c credential.helper= -c credential.helper=!"gh" auth git-credential push --set-upstream <remote> HEAD
   ```
   On a bare repo, the same credential flags with `push <remote> --mirror` (`git/client.go:911-921`, `create.go:682-705`). This uses `AllMatchingCredentialsPattern`.

---

### `gh repo view [<repo>] [--branch B] [--json fields] [--web]`

Source: `pkg/cmd/repo/view/view.go`, `pkg/cmd/repo/view/http.go`.

**Feature detection / IsEnterprise:** none; GHES and github.localhost are identical.

Order of requests (`view.go:80-138`):
1. Resolve the target repo:
   - No arg: H6 (possibly `RepositoryNetwork`).
   - Arg without `/`: H1 to get the login, giving `login/arg`.
   - `OWNER/REPO` or `HOST/OWNER/REPO`: no request.
2. **H5** `RepositoryInfo`:
   - Default (no `--json`): fields `name,owner,description` (`view.go:78`), so the selection is `repository(owner: $owner, name: $name) {name,owner{id,login},description}`.
   - `--json f1,f2`: exactly the requested fields, mapped as in the table below.
   - This query is also sent with `--web`.
3. README (only when neither `--json` nor `--web` is given): **`GET repos/{owner}/{repo}/readme`**, plus `?ref=<branch>` with `--branch` (`http.go:26-74`).
   - Reads `name`, `content` and `html_url`.
   - `content` must be standard base64. Embedded `\n` is fine because Go's decoder skips CR and LF.
   - 404 means "This repository does not have a README". Any other error is fatal.
   - Markdown is rendered if `name` ends in `.md`, `.markdown`, `.mdown` or `.mkdown`. `html_url` is used as the base URL for relative links.

Output:
- Non-TTY: `name:\tOWNER/REPO\ndescription:\t<desc>\n--\n<readme>\n`.
- TTY: a template that also includes `View this repository on GitHub: {HostPrefix}OWNER/REPO[/tree/<branch>]`.
- `--web` opens `http(s)://HOST/OWNER/REPO`.

**`--json` fields** (`api/query_builder.go:476-553`). Each field maps to a GraphQL selection inside `repository{...}` (`api/query_builder.go:555-610`). Any field not listed in the table is emitted verbatim as a scalar.

| JSON field | GraphQL selection |
|---|---|
| id, name, nameWithOwner, description, homepageUrl, openGraphImageUrl, usesCustomOpenGraphImage, url, sshUrl, mirrorUrl, securityPolicyUrl, createdAt, pushedAt, updatedAt, archivedAt, isBlankIssuesEnabled, isSecurityPolicyEnabled, hasIssuesEnabled, hasProjectsEnabled, hasWikiEnabled, hasDiscussionsEnabled, mergeCommitAllowed, squashMergeAllowed, rebaseMergeAllowed, forkCount, stargazerCount, deleteBranchOnMerge, diskUsage, isArchived, isEmpty, isFork, isInOrganization, isMirror, isPrivate, visibility, isTemplate, isUserConfigurationRepository, viewerCanAdminister, viewerDefaultCommitEmail, viewerDefaultMergeMethod, viewerHasStarred, viewerPermission, viewerPossibleCommitEmails, viewerSubscription | the same name as a scalar |
| owner | `owner{id,login}` |
| parent | `parent{id,name,owner{id,login}}` |
| templateRepository | `templateRepository{id,name,owner{id,login}}` |
| codeOfConduct | `codeOfConduct{key,name,url}` |
| contactLinks | `contactLinks{about,name,url}` |
| fundingLinks | `fundingLinks{platform,url}` |
| licenseInfo | `licenseInfo{key,name,nickname}` |
| repositoryTopics | `repositoryTopics(first:100){nodes{topic{name}}}` |
| issueTemplates | `issueTemplates{name,title,body,about}` |
| pullRequestTemplates | `pullRequestTemplates{body,filename}` |
| labels | `labels(first:100){nodes{id,color,name,description}}` |
| languages | `languages(first:100){edges{size,node{name}}}` |
| primaryLanguage | `primaryLanguage{name}` |
| latestRelease | `latestRelease{publishedAt,tagName,name,url}` |
| milestones | `milestones(first:100,states:OPEN){nodes{number,title,description,dueOn}}` |
| assignableUsers | `assignableUsers(first:100){nodes{id,login,name}}` |
| mentionableUsers | `mentionableUsers(first:100){nodes{id,login,name}}` |
| projects | `projects(first:100,states:OPEN){nodes{id,name,number,body,resourcePath}}` (classic Projects v1; github.com now errors on it) |
| projectsV2 | `projectsV2(first:100,query:"is:open"){nodes{id,number,title,resourcePath,closed,url}}` |
| watchers | `watchers{totalCount}` |
| issues | `issues(states:OPEN){totalCount}` |
| pullRequests | `pullRequests(states:OPEN){totalCount}` |
| defaultBranchRef | `defaultBranchRef{name}` |

There is no pagination; the `first:100` connections are fetched once. Export shaping (`api/export_repo.go:7-53`):
- `parent` and `templateRepository` become `{id,name,owner}`.
- `labels`, `assignableUsers`, `mentionableUsers`, `milestones` and `projects` are flattened to their `nodes`.
- `languages` becomes the `edges` list.
- `repositoryTopics` becomes `[{name}]`.

---

### `gh repo clone <repo> [<dir>] [-- gitflags] [--upstream-remote-name N | --no-upstream]`

Source: `pkg/cmd/repo/clone/clone.go`.

**Feature detection / IsEnterprise:** none.

1. Argument parsing (`clone.go:124-164`):
   - The argument contains `:`: it is parsed as a URL (`git.ParseURL`), stripped to `/owner/repo`, and the protocol is the URL scheme (`git+ssh` becomes `ssh`). For example `http://github.localhost/o/r` gives protocol `http`.
   - `OWNER/REPO`: the protocol is config `git_protocol` for that host (default `https`).
   - Bare `REPO`: send H1 (login), giving `login/REPO`.
   - A `.wiki` suffix is stripped for the API lookup.
2. **H4** `RepositoryInfo` (canonical capitalization plus parent info). Reads `name`, `owner.login`, `sshUrl`, `hasWikiEnabled`, `parent{...}` and `parent.defaultBranchRef.name`.
3. Clone URL (`clone.go:245-253`):
   - If protocol ≠ `ssh`: `FormatRemoteURL(canonicalRepo, protocol)`. That is `https://HOST/OWNER/REPO.git` on GHES and `http://github.localhost/OWNER/REPO.git` on localhost; `HostPrefix` decides the scheme, not the protocol string.
   - If protocol is `ssh`: **the API's `sshUrl` is used verbatim**. If it is empty, gh fails with "invalid API response: repository O/R is missing sshUrl".
   - Wiki: if `hasWikiEnabled` is false, gh errors. Otherwise `.git` is replaced with `.wiki.git`.
4. `git -c credential.<pattern>.helper= -c credential.<pattern>.helper=!"gh" auth git-credential clone [gitflags] URL [dir]`, where `<pattern>` is `https://HOST` or `http://github.localhost` (`git/client.go:923-955,135-163`).
5. If `parent != null` (fork):
   - `--no-upstream`: `git -C dir config --add remote.origin.gh-resolved base`.
   - Otherwise, with upstream URL = `remoteURL(parent, git_protocol)` (parent `sshUrl` for ssh) and name = `upstream`, `--upstream-remote-name`, or the parent owner when that is `@owner`:
     - `git -C dir remote add -t <parent.defaultBranchRef.name> <name> <upstreamURL>`
     - `git -C dir -c credential.helper= -c credential.helper=!"gh" auth git-credential fetch <name>`
     - `git -C dir remote set-branches <name> *`
     - `git -C dir config --add remote.<name>.gh-resolved base` (`clone.go:198-241`)

     The server must return the parent's `defaultBranchRef{name}`; it is passed as `-t`.

---

### `gh repo list [<owner>] [--limit N] [--visibility public|private|internal] [--fork|--source] [--language L] [--topic T...] [--archived|--no-archived] [--json ...]`

Source: `pkg/cmd/repo/list/list.go`, `pkg/cmd/repo/list/http.go`.

**Feature detection (`RepositoryFeatures`, `list.go:132-145`, `internal/featuredetection/feature_detection.go:294-329`):**
- **github.localhost / github.com**: `IsEnterprise` is false, so all features are assumed and no request is sent. Default fields always include `visibility`. This is the simpler path.
- **GHES**: one extra request is sent first, through an HTTP client that adds an `X-GH-CACHE-TTL` header (24h, cached locally):
  ```graphql
  query Repository_fields{Repository: __type(name: "Repository"){fields(includeDeprecated: true){name}}}
  ```
  If the returned `fields[].name` list contains `visibility`, `visibility` is added to the default fields. `pullRequestTemplates` and `autoMergeAllowed` are also looked for, but they are unused here.

Default fields: `nameWithOwner,description,isPrivate,isFork,isArchived,createdAt,pushedAt` plus `visibility` when available (`list.go:117`). With `--json`, exactly the requested fields from the same `RepositoryFields` list as `repo view`, using the same mapping table.

**Path A: owner listing.** Used when none of `--language`, `--archived`, `--no-archived` or `--topic` is given and visibility is not `internal` (`http.go:32-116`):
```graphql
query RepositoryList($perPage:Int!,$endCursor:String,$privacy:RepositoryPrivacy,$fork:Boolean[,$owner:String!]) {
		<OWNER_CONN> {
			login
			repositories(first: $perPage, after: $endCursor, privacy: $privacy, isFork: $fork, ownerAffiliations: OWNER, orderBy: { field: PUSHED_AT, direction: DESC }) {
				nodes{<RepositoryGraphQL(fields)>}
				totalCount
				pageInfo{hasNextPage,endCursor}
			}
		}
	}
```
- `<OWNER_CONN>` is `repositoryOwner: viewer` when no owner arg is given. This is an **alias**: the `viewer` field is aliased as `repositoryOwner`, so `User.repositories` must accept these arguments. With an owner arg it is `repositoryOwner(login: $owner)`, and `,$owner:String!` is appended to the variable list.
- The default selection, as built, is `nodes{nameWithOwner,description,isPrivate,isFork,isArchived,createdAt,pushedAt,visibility}`.
- Variables: `perPage = min(limit,100)`, constant across pages.
  - `privacy` (`PUBLIC` or `PRIVATE`) is present only with `--visibility public|private`.
  - `fork` is `true` with `--fork`, `false` with `--source`, and absent otherwise.
  - `owner` is present only with an owner arg.
  - `endCursor` is absent on the first page.
  - **Absent nullable variables must be treated as null**, meaning no filter.
- Pagination: loop while `pageInfo.hasNextPage`, setting `endCursor`. Stop once `limit` items have been collected.
- Reads `repositoryOwner.login` and `repositories.totalCount`. If an owner arg was given and the response has `repositoryOwner: null` (so the login is empty), gh errors with `the owner handle "X" was not recognized as either a GitHub user or an organization` (`list.go:166-168`).

**Path B: search.** Used for `--language`, `--archived`, `--no-archived`, `--topic` or `--visibility internal` (`http.go:118-211`):
```graphql
query RepositoryListSearch($query:String!,$perPage:Int!,$endCursor:String) {
		search(type: REPOSITORY, query: $query, first: $perPage, after: $endCursor) {
			repositoryCount
			nodes{...on Repository{<RepositoryGraphQL(fields)>}}
			pageInfo{hasNextPage,endCursor}
		}
	}
```
- The `query` string is `sort:updated-desc` followed by the qualifiers, sorted by key and space-joined (`pkg/search/query.go:126-136,298-338`). Values that contain spaces or quotes are double-quoted.
  - `archived:true|false` with `--archived` or `--no-archived`.
  - `fork:true` by default, `fork:only` with `--fork`, `fork:false` with `--source`. A `fork:` qualifier is always present.
  - `is:<visibility>` if set.
  - `language:L`.
  - `topic:T`, repeated and sorted.
  - `user:<owner or @me>`.
- Example: `sort:updated-desc archived:false fork:true is:internal language:go topic:cli user:@me`.
- Reads `repositoryCount` (as total) and `nodes`. The owner is derived from the first node's `nameWithOwner`, so **`nameWithOwner` should be selectable**; it is in the default fields. Pagination works as in Path A.
- If `limit > 1000`, gh warns that the Search API is capped at 1000 results.

Output columns: `NAME`=`nameWithOwner`, `DESCRIPTION`, `INFO` (`visibility` lowercased, else private/public from `isPrivate`, plus `fork` and `archived`), `UPDATED`=`pushedAt` or, if null, `createdAt`.

---

### `gh repo fork [<repo>] [--clone] [--remote] [--remote-name N] [--org O] [--fork-name F] [--default-branch-only] [-- gitflags]`

Source: `pkg/cmd/repo/fork/fork.go`, `api/queries_repo.go:595-675`.

**Feature detection / IsEnterprise:** none.

1. Repo to fork:
   - No arg: H6 (in-parent mode).
   - `http(s)://` URL or `git@` URL: parsed locally.
   - Otherwise `[HOST/]OWNER/REPO`. Note there is no H1 here: a bare name is an error.
2. **`POST repos/{owner}/{repo}/forks`** (`api/queries_repo.go:595-642`).
   - Body is JSON with a trailing newline. It is `{}` when no options are set; otherwise it contains `"organization":"O"` (with `--org`), `"name":"F"` (with `--fork-name`), and `"default_branch_only":true` (with `--default-branch-only`).
   - Reads `node_id`, `name`, `owner.login` and `created_at`. GitHub returns 202, but any 2xx is accepted.
   - If the response owner and name equal the source repo (case-insensitively), gh errors: "cannot be forked. A single user account cannot own both a parent and fork."
3. "Already exists" detection: if `now - created_at > 1 minute`, gh prints `OWNER/NAME already exists` (not an error). Otherwise it prints `Created fork` (TTY) or the fork URL `HostPrefix+OWNER/NAME` (non-TTY) (`fork.go:226-242`). **`created_at` must be a valid RFC 3339 timestamp of the actual creation time.**
4. If `--fork-name` is given and the returned `name` ≠ the normalized fork name, gh sends **`PATCH repos/{forkOwner}/{forkName}`** with body `{"name":"F"}\n`. It reads `node_id`, `name`, `owner.login` and `created_at` (`api/queries_repo.go:645-675`).
5. In-parent mode (no arg), with `--remote` or a prompt (`fork.go:270-345`):
   - There is no HTTP. Remotes come from `git remote -v`.
   - The protocol is the scheme of the existing remote for the parent unless `git_protocol` is user-configured.
   - If a remote already points at the fork, gh stops.
   - Otherwise, if a remote named `--remote-name` (default `origin`) exists, it is renamed with `git remote rename origin upstream`. Then `git remote add <name> <FormatRemoteURL(fork, protocol)>`.
6. Arg mode with `--clone` or a prompt (`fork.go:346-399`). **There is no API polling for fork readiness.** Instead:
   - gh runs `git ... clone [gitflags] <forkURL>`. On git exit code 128 it retries up to 3 times with a constant **2s** back-off, so at most about 6s after the first failure.
   - Then:
     - `git -C dir remote add upstream <FormatRemoteURL(source, protocol)>`
     - `git -C dir config --add remote.upstream.gh-resolved base`
     - `git -C dir -c credential.helper= -c credential.helper=!"gh" auth git-credential fetch upstream`
   - Clone URLs follow `FormatRemoteURL`: `https://HOST/O/R.git`, `http://github.localhost/O/R.git`, or `git@HOST:O/R.git`. The `sshUrl` from the API is not used here.

   So the fork's git repository must be clonable within about 6s of the POST response; a server that creates forks synchronously avoids the problem.

---

### `gh repo delete [<repo>] [--yes]`

Source: `pkg/cmd/repo/delete/delete.go`, `pkg/cmd/repo/delete/http.go`.

**Feature detection / IsEnterprise:** none.

1. Target repo:
   - No arg: H6. `--yes` is ignored in this case, and a non-interactive run fails.
   - Arg without `/`: H1 against `ghauth.DefaultHost()`.
   - Otherwise `[HOST/]OWNER/REPO`.
2. There is no pre-flight GET and no scope check before the request; confirmation is by typing the name.
3. **`DELETE repos/{owner}/{repo}`** (`http.go:11-33`), sent with redirects **not followed**.
   - Success is any 2xx (GitHub returns 204, no body).
   - **301, 307 or 308** produce "Failed to delete repository: O/R has changed name or transferred ownership" (`delete.go:124-135`).
   - On a 4xx other than 422, the endpoint scopes `delete_repo` are merged into `X-Accepted-Oauth-Scopes`. If the response has `X-Oauth-Scopes` lacking `delete_repo`, gh prints a hint to run `gh auth refresh -h HOST -s delete_repo` (`api/client.go:89-96,306-314`). For this hint to appear, a server should return 403 or 404 with `X-Oauth-Scopes`.

---

### `gh label list [--limit N] [--search Q] [--sort created|name] [--order asc|desc] [--json ...] [--web]`

Source: `pkg/cmd/label/list.go`, `pkg/cmd/label/http.go`.

**Feature detection / IsEnterprise:** none. The repo comes from `-R`, `GH_REPO`, or H6.

`--web` opens `HostPrefix OWNER/REPO/labels` with no API call. Otherwise gh sends one query (`http.go:68-127`). The fragment is built from the requested fields:
```graphql
fragment label on Label{color,description,name}
	query LabelList($owner: String!, $repo: String!, $limit: Int!, $endCursor: String, $query: String, $orderBy: LabelOrder) {
		repository(owner: $owner, name: $repo) {
			labels(first: $limit, after: $endCursor, query: $query, orderBy: $orderBy) {
				totalCount,
				nodes {
					...label
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}
```
- Default fragment fields are `color,description,name` (`http.go:19-23`). With `--json`, the fields are exactly the requested ones in user order. Available fields are `color, createdAt, description, id, isDefault, name, updatedAt, url` (`shared.go:9-18`), each a scalar `Label` field of the same name.
- Variables: `{"owner":"O","repo":"R","orderBy":{"direction":"ASC|DESC","field":"CREATED_AT|NAME"},"query":"<search or empty string>","limit":N}`, plus `endCursor` on later pages.
  - `orderBy` defaults to `{"direction":"ASC","field":"CREATED_AT"}` and **is sent even with `--search`**.
  - **`query` is always sent, as `""` when there is no search**. The server must treat `""` as no filter.
  - When searching, the help text says results are ordered by best match.
- Page size: `limit = min(remaining, 100)` per request, where remaining is `--limit` (default 30) minus the labels already fetched. gh loops while `hasNextPage`.
- Reads `totalCount` (for the header "Showing X of N labels in O/R"), `nodes`, and `pageInfo`. Zero labels is reported as a NoResults error with exit code 0.
- Table columns: `NAME`, `DESCRIPTION`, `COLOR` as `#<color>`. `color` is 6 hex digits without `#`.

---

### `gh label create <name> [--color C] [--description D] [--force]`

Source: `pkg/cmd/label/create.go`.

**Feature detection / IsEnterprise:** none. The repo comes from `-R`, `GH_REPO`, or H6.

1. `color` has any leading `#` stripped. If it is empty, a random value from a fixed list of 16 hex colors is used (the last entry, `D4C5F9`, is never chosen) (`create.go:22-39,105-108`).
2. **`POST repos/{owner}/{repo}/labels`** (`create.go:129-160`). Body (from `json.Marshal` of a map, so keys are sorted and `description` is always present, even as `""`):
   ```json
   {"color":"E99695","description":"","name":"bug"}
   ```
   The response body is ignored. GitHub returns 201; any 2xx is accepted.
3. "Already exists" detection: HTTP **422** with a JSON body whose `errors` array has **exactly one** element with `field == "name"` and `code == "already_exists"` (`create.go:191-193`). For example: `{"message":"Validation Failed","errors":[{"resource":"Label","code":"already_exists","field":"name"}]}`. The response `Content-Type` must match `[/+]json($|;)`, or the errors are not parsed.
   - Without `--force`: gh errors with `label with name "X" already exists; use --force to update its color and description`.
4. `--force` with "already exists": **`PATCH repos/{owner}/{repo}/labels/{name}`**, where the name is a percent-encoded path segment (`create.go:162-189`).
   - Body contains only the non-empty keys among `color`, `description` and `new_name`. `color` is always present, because a random one was chosen if none was given.
   - Example: `{"color":"E99695","description":"Something"}`.
   - The response is ignored. The same 422 `already_exists` check is applied.

## 3.2 Issue commands

Scope: `gh issue create | list | view | comment | close | reopen | edit | status` (gh v2.102.0).
All citations are `path:line` relative to the cli repo root. Base-repo resolution is described
elsewhere; every command below starts after `BaseRepo()` has produced `OWNER/REPO` on `HOST`.

### 0. Conventions used in this section

**Two GraphQL wire formats.** gh builds GraphQL requests in two ways and the text differs:

1. *String queries* (`client.GraphQL(...)`, api/client.go:108): the query text is a Go raw
   string literal sent verbatim, including the leading newline, tab indentation and newlines.
   Interpolated fragments (from `api.IssueGraphQL`) are single-line. Body is always
   `{"query": "...", "variables": {...}}`; `variables` is always present (go-gh
   pkg/api/graphql_client.go:69), JSON `null` if Go passed `nil`.
2. *Struct queries* (`client.Query(name, ...)` / `client.Mutate(name, ...)`, api/client.go:122-142):
   generated by `github.com/cli/shurcooL-graphql` v0.0.4 from Go structs. Rules:
   - text is `query NAME(VARS){...}` / `mutation NAME(VARS){...}`; if no variables: `query NAME{...}`;
   - VARS = variables sorted by name, each `$name:Type`, **no separators** between them
     (e.g. `$endCursor:String$name:String!$owner:String!`);
   - Go pointer => nullable type (no `!`); non-pointer => `!`; slices => `[Elem!]!`;
     type name = Go type name (`githubv4.String`=>`String`, `githubv4.Int`=>`Int`,
     `githubv4.ID`=>`ID`, local struct `CloseIssueInput`=>`CloseIssueInput`);
   - selection sets are `{a,b,c}` (comma separated, no spaces); field name = `graphql:"..."`
     tag verbatim, else lowerCamelCase of the Go field name (`ID`=>`id`, `URL`=>`url`,
     `DatabaseID`=>`databaseId`, `OptionID`=>`optionId`);
   - `variables` key omitted from the body when there are none.
   - a variable whose dynamic Go type is plain `string` is declared `ID` (shurcooL hack,
     query.go:82-84); `githubv4.ID(x)` stored in `map[string]any` is a plain string, so
     `"id": githubv4.ID(...)` ⇒ `$id:ID!`.
   The struct-query texts below are reconstructed by applying these rules (verified against
   cli/shurcooL-graphql v0.0.4 query.go and githubv4 input.go @48295856cce7); spacing inside
   `graphql:"..."` tags (e.g. `first: 100, after: $endCursor`) is reproduced verbatim.

**Header.** Every GraphQL request carries `GraphQL-Features: merge_queue` (api/client.go:23-24,111).

**Client-side caching.** Feature-detection requests (`Issue_fields`, `SearchType_enumValues`,
`GET meta`) are sent through `api.NewCachedHTTPClient(httpClient, 24h)`
(api/http_client.go:135-150), which only adds an `X-GH-CACHE-TTL` header; go-gh then caches the
response on disk. A server may therefore not see these requests on every invocation.

**Feature-detection branches (`internal/featuredetection/feature_detection.go`).**

| Detector call | github.com / `github.localhost` (`IsEnterprise==false`) | GHES |
|---|---|---|
| `IssueFeatures()` :189 | no request; `ApiActorsSupported=true`, `IssueRelationshipsSupported=true` | `ApiActorsSupported=false`; POST graphql `query Issue_fields{Issue: __type(name: "Issue"){fields(includeDeprecated: true){name}}}`; `IssueRelationshipsSupported` = any field named `blockedBy` |
| `ProjectsV1()` :335 | no request; unsupported | `GET /api/v3/meta`, reads `installed_version`; supported iff `< 3.17.0` |
| `SearchFeatures()` :399 | POST graphql `query SearchType_enumValues{SearchType: __type(name: "SearchType"){enumValues(includeDeprecated: true){name}}}` (yes, even on github.com) | `GET /api/v3/meta`; if `installed_version >= 3.18.0` same introspection query as left; else no query and `AdvancedIssueSearchAPI=false` |

`SearchType_enumValues` reads value names: `ISSUE_ADVANCED` => opt-in (gh sends `type: ISSUE_ADVANCED`),
`ISSUE_SEMANTIC`, `ISSUE_HYBRID` (not used by `issue list`).

**Simplest server choice:** use a non-enterprise hostname (`github.localhost`). Then `create`,
`view`, `edit` send no detection requests; only `issue list` with search filters sends the
`SearchType` introspection. Do **not** include `ISSUE_ADVANCED` in `SearchType` and gh will send
`type: ISSUE` with advanced-syntax query strings.

**Error matching gh relies on.** gh's `GraphQLError.Match(type, path)` (go-gh pkg/api/errors.go:73)
requires *all* error items to have `type == T` and dotted `path` matching. Relevant cases:
- `FindIssueOrPR` (pkg/cmd/issue/shared/lookup.go:177-191): an error of `type:"NOT_FOUND"`,
  `path:["repository","issue"]` combined with `data.repository.hasIssuesEnabled == false`
  => "the 'O/R' repository has disabled issues"; otherwise the error message is printed.
  GitHub's message is `Could not resolve to an issue or pull request with the number of N.`
- `FORBIDDEN` on `repository.issue.projectCards.*` => partial load (GHES classic projects).
- ProjectsV2 list errors whose message contains any of: `field requires one of the following scopes: ['read:project']`,
  `Field 'projectsV2' doesn't exist on type 'User'|'Repository'|'Organization'`,
  `Field 'projectItems' doesn't exist on type 'Issue'|'PullRequest'`, `Resource not accessible by`
  are ignored (api/queries_projects_v2.go:11-19,330-340). Organization lookups that fail with a
  message containing `Could not resolve to an Organization` are ignored (api/queries_repo.go:25,1587-1593).

---

### 1. Shared building blocks

#### B1. `IssueByNumber` – fetch one issue-or-PR (`FindIssueOrPR`, pkg/cmd/issue/shared/lookup.go:136-209)

Used by view, comment, close, reopen, edit. Input: list of gh field names `F`.

1. `F` is de-duplicated preserving insertion order (pkg/set/string_set.go). If `F` contains
   `projectItems` it is **removed** and `number` is appended if absent (lookup.go:140-147);
   project items are then fetched by B2.
2. Request — POST graphql, string query (lookup.go:156-166):

```graphql

	query IssueByNumber($owner: String!, $repo: String!, $number: Int!) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issue: issueOrPullRequest(number: $number) {
				__typename
				...on Issue{<IssueGraphQL(F)>}
				...on PullRequest{<PullRequestGraphQL(F)>}
			}
		}
	}
```
   (Leading `\n`, one-tab indent on the `query` line as in source.) Variables:
   `{"owner":"OWNER","repo":"REPO","number":123}` (number is a JSON integer).
   `PullRequestGraphQL(F)` = `IssueGraphQL(F minus issue-only fields)` where issue-only =
   `isPinned, stateReason, closedByPullRequestsReferences, issueType, parent, subIssues,
   subIssuesSummary, blockedBy, blocking` (api/query_builder.go:339-349,469-474).
3. Reads `data.repository.hasIssuesEnabled`, `data.repository.issue` (`__typename` decides
   Issue vs PullRequest: `IsPullRequest()` == `__typename=="PullRequest"`, api/queries_issue.go:125-132).
   `issue == null` with no error => error "issue was not found but GraphQL reported no error".
4. If `projectItems` was requested: B2.

The server **must implement `Repository.issueOrPullRequest(number:)`** returning the
`IssueOrPullRequest` union and support `__typename`.

#### B2. `IssueProjectItems` – paginate project v2 items (api/queries_projects_v2.go:65-133)

```graphql
query IssueProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){issue(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}
```
Variables first page: `{"owner":"O","name":"R","number":N,"endCursor":null}`; loops while
`pageInfo.hasNextPage`, setting `endCursor`. Null nodes are skipped. Errors matching the
ignorable list above are swallowed. Note it uses `repository.issue(number:)`, **not**
`issueOrPullRequest`, so on a PR number GitHub returns a NOT_FOUND error which is not ignorable.

#### B3. `UserCurrent` (api/queries_user.go:7-15)
```graphql
query UserCurrent{viewer{login}}
```
No variables. Used to expand `@me` and by `issue status`.

#### B4. RepoMetadata sub-queries (api/queries_repo.go:986-1092; run concurrently via errgroup)

Which sub-queries run depends on the `RepoMetadataInput` booleans set by the caller.

- Labels — `RepoLabels` (:1421-1456):
  ```graphql
  query RepositoryLabelList($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){labels(first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name},pageInfo{hasNextPage,endCursor}}}}
  ```
  Paginates to exhaustion. Name→ID matching is **case-insensitive** (`strings.EqualFold`, :809-825); unknown => `could not add label: 'x' not found`.
- Milestones — `RepoMilestones(state="open")` (:1464-1512):
  ```graphql
  query RepositoryMilestoneList($endCursor:String$name:String!$owner:String!$states:[MilestoneState!]!){repository(owner: $owner, name: $name){milestones(states: $states, first: 100, after: $endCursor){nodes{id,title},pageInfo{hasNextPage,endCursor}}}}
  ```
  Variables `{"owner":..,"name":..,"states":["OPEN"],"endCursor":null}`. Title match case-insensitive (:942-949). Only OPEN milestones can be assigned by title.
- Assignable users (legacy / GHES path only, i.e. when `ApiActorsSupported` is false) — `RepoAssignableUsers` (:1237-1284):
  ```graphql
  query RepositoryAssignableUsers($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){assignableUsers(first: 100, after: $endCursor){nodes{id,login,name},pageInfo{hasNextPage,endCursor}}}}
  ```
  Login match case-insensitive (`MembersToIDs`, :754-788).
- Assignable actors (actors path; only reached from interactive flows for issues) — `RepoAssignableActors` (:1288-1350):
  ```graphql
  query RepositoryAssignableActors($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){suggestedActors(first: 100, after: $endCursor, capabilities: CAN_BE_ASSIGNED){nodes{... on User{id,login,name,__typename},... on Bot{id,login,__typename}},pageInfo{hasNextPage,endCursor}}}}
  ```
- Projects v2 — `v2Projects` (:1557-1617) runs 3 queries concurrently and de-duplicates by `id`:
  ```graphql
  query UserProjectV2List($endCursor:String$query:String!){viewer{projectsV2(first: 100, orderBy: {field: TITLE, direction: ASC}, after: $endCursor, query: $query){nodes{id,title,number,resourcePath,closed,url},pageInfo{hasNextPage,endCursor}}}}
  query RepositoryProjectV2List($endCursor:String$name:String!$owner:String!$query:String!){repository(owner: $owner, name: $name){projectsV2(first: 100, orderBy: {field: TITLE, direction: ASC}, after: $endCursor, query: $query){nodes{id,title,number,resourcePath,closed,url},pageInfo{hasNextPage,endCursor}}}}
  query OrganizationProjectV2List($endCursor:String$owner:String!$query:String!){organization(login: $owner){projectsV2(first: 100, orderBy: {field: TITLE, direction: ASC}, after: $endCursor, query: $query){nodes{id,title,number,resourcePath,closed,url},pageInfo{hasNextPage,endCursor}}}}
  ```
  (one request each; `query` variable is always `"is:open"`; api/queries_projects_v2.go:208-322).
  For a user-owned repo the org query should fail with a message containing
  `Could not resolve to an Organization` (ignored) or return `organization: null` (also fine).
  Title match case-insensitive (:865-873).
- Projects classic (GHES `< 3.17` only, `ProjectsV1Supported`) — `v1Projects` (:1517-1551), concurrently:
  ```graphql
  query RepositoryProjectList($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){projects(states: [OPEN], first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name,number,resourcePath},pageInfo{hasNextPage,endCursor}}}}
  query OrganizationProjectList($endCursor:String$owner:String!){organization(login: $owner){projects(states: [OPEN], first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name,number,resourcePath},pageInfo{hasNextPage,endCursor}}}}
  ```
  (api/queries_repo.go:1102-1137, api/queries_org.go:10-44). A project title is matched first against classic projects, then v2.

#### B5. `UpdateProjectV2Items` (api/queries_projects_v2.go:35-62)

String mutation built dynamically; skipped entirely when there is nothing to add/remove.
For each add `i` (0-based, 3-digit) and then each delete:
```graphql
mutation UpdateProjectV2Items($input_000: AddProjectV2ItemByIdInput! $input_001: DeleteProjectV2ItemInput!) {add_000: addProjectV2ItemById(input: $input_000) { item { id } } delete_001: deleteProjectV2Item(input: $input_001) { deletedItemId }}
```
(declarations joined by a single space; selections joined by a single space). Variables:
`{"input_000":{"contentId":"<issue node id>","projectId":"<project id>"},"input_001":{"itemId":"<project item id>","projectId":"<project id>"}}`.
Iteration order over Go maps is random, so the index↔project assignment is not stable. Response is not read (`data` ignored).

#### B6. Issue type / relationship helpers (api/queries_issue.go:505-802; pkg/cmd/issue/shared/lookup.go:211-248)

- `RepositoryIssueTypes` (string query, :748-773) — run for `--type NAME` (name matched case-insensitively):
  ```graphql

	query RepositoryIssueTypes($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			issueTypes(first: 50) {
				nodes { id, name, description, color }
			}
		}
	}
  ```
- `IssueNodeID` (string query, :776-802) — once per `--parent/--blocked-by/--blocking/--add-sub-issue/...` reference (number or `http(s)://HOST/O/R/issues|pull/N` URL on the same host):
  ```graphql

	query IssueNodeID($owner: String!, $name: String!, $number: Int!) {
		repository(owner: $owner, name: $name) {
			issue(number: $number) {
				id
			}
		}
	}
  ```
- Deferred mutations (`DeferredUpdateIssue`, :668-745), all sent **concurrently**, errors joined:
  ```graphql
  mutation UpdateIssueIssueType($input:UpdateIssueIssueTypeInput!){updateIssueIssueType(input: $input){issue{id}}}
  mutation AddSubIssue($input:AddSubIssueInput!){addSubIssue(input: $input){issue{id}}}
  mutation RemoveSubIssue($input:RemoveSubIssueInput!){removeSubIssue(input: $input){issue{id}}}
  mutation AddBlockedBy($input:AddBlockedByInput!){addBlockedBy(input: $input){issue{id}}}
  mutation RemoveBlockedBy($input:RemoveBlockedByInput!){removeBlockedBy(input: $input){issue{id}}}
  ```
  Inputs: `{"issueId":ID,"issueTypeId":ID|null}` (null clears); `{"issueId":PARENT,"subIssueId":CHILD,"replaceParent":bool}`;
  `{"issueId":PARENT,"subIssueId":CHILD}`; `{"issueId":BLOCKED,"blockingIssueId":BLOCKER}`.
  "This issue blocks X" (`--blocking`) is sent as `addBlockedBy{issueId: X, blockingIssueId: THIS}`.

#### B7. `--json` field map (`api.IssueFields`, api/query_builder.go:314-351; selection from `IssueGraphQL`, :385-465)

Fields available to `--json` for `issue list`, `view`, `status`:

| JSON field | GraphQL selection emitted on `Issue` |
|---|---|
| `assignees` | `assignees(first:100){nodes{id,login,name,databaseId},totalCount}` |
| `author` | `author{login,...on User{id,name}}` |
| `body` | `body` |
| `closed` | `closed` |
| `comments` | `comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount}` |
| `createdAt` | `createdAt` |
| `closedAt` | `closedAt` |
| `id` | `id` |
| `labels` | `labels(first:100){nodes{id,name,description,color},totalCount}` |
| `milestone` | `milestone{number,title,description,dueOn}` |
| `number` | `number` |
| `projectCards` | `projectCards(first:100){nodes{project{name}column{name}},totalCount}` (classic; note no comma between `project{name}` and `column{name}`) |
| `projectItems` | `projectItems(first:100){nodes{id, project{id,title}, status:fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},totalCount}` (inline in list/status; view uses B2 instead) |
| `reactionGroups` | `reactionGroups{content,users{totalCount}}` |
| `state` | `state` |
| `title` | `title` |
| `updatedAt` | `updatedAt` |
| `url` | `url` |
| `isPinned` | `isPinned` |
| `stateReason` | `stateReason` |
| `closedByPullRequestsReferences` | `closedByPullRequestsReferences(first: 100) {nodes {id,number,url,repository {id,name,owner {id,login}}}pageInfo{hasNextPage,endCursor}}` |
| `issueType` | `issueType{id,name,description,color}` |
| `parent` | `parent{id,number,title,url,state,repository{nameWithOwner}}` |
| `subIssues` | `subIssues(first:100){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount}` |
| `subIssuesSummary` | `subIssuesSummary{total,completed,percentCompleted}` |
| `blockedBy` | `blockedBy(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount}` |
| `blocking` | `blocking(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount}` |

Internal pseudo-fields also used: `lastComment` → `comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount}`;
`assignedActors` → `assignedActors(first: 10) {nodes {...on User {id,login,name,__typename}...on Bot {id,login,__typename}},totalCount}` (no comma between the two inline fragments);
`repository` → `repository{id,name,nameWithOwner,databaseId,viewerPermission}` (only with `--attach`).
Fields are emitted in the order the user listed them, joined with `,`.
Output shaping (api/export_pr.go:8-118): `author` → `{"is_bot":false,"login","id","name"}` or, when `id` is empty (non-User actor, e.g. Bot), `{"is_bot":true,"login":"app/<login>"}` (api/queries_issue.go:294-307); `reactionGroups` drops groups with `users.totalCount==0`; `projectItems` → `[{"status":{"optionId","name"},"title"}]`; `parent/subIssues/blockedBy/blocking` drop `repository`.

---

### `gh issue create`

Source: pkg/cmd/issue/create/create.go. Non-interactive requires `--title` and `--body`/`--body-file` (create.go:154-158).

Requests, in order:

1. **Feature detection** (create.go:215-224): `ProjectsV1()` then `IssueFeatures()`.
   - github.localhost / github.com: none.
   - GHES: `GET /api/v3/meta` and `query Issue_fields{...}` (see §0).
2. **`@me` expansion** (create.go:235-241, pkg/cmd/pr/shared/editable.go:197-214, params.go:284-327):
   if any `--assignee` equals `@me`, one `UserCurrent` (B3) (memoised). `@copilot` (case-insensitive)
   is replaced with login `copilot-swe-agent` **only** when `ApiActorsSupported` (github.com path);
   on GHES it stays literally `@copilot` and later fails ID resolution. Assignees are de-duplicated.
3. **Repository info** (create.go:290, api/queries_repo.go:321-357), string query:
   ```graphql

	query IssueRepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			id
			databaseId
			name
			owner { login }
			hasIssuesEnabled
			viewerPermission
		}
	}
   ```
   Reads `id` (used as `repositoryId`), `hasIssuesEnabled` (false => error), `databaseId`/`viewerPermission` (only for `--attach`).
   `repository: null` without errors => gh synthesises NOT_FOUND.
4. **Metadata resolution** (create.go:460, pkg/cmd/pr/shared/params.go:59-125) — only if at least one of
   `--assignee/--label/--project/--milestone` given (`HasMetadata`, state.go:55-61). Concurrent B4 sub-queries:
   | flag | github.localhost/github.com | GHES |
   |---|---|---|
   | `--label` | `RepositoryLabelList` | same |
   | `--milestone` | `RepositoryMilestoneList` (OPEN) | same |
   | `--project` | 3× ProjectV2 list queries | same + classic `RepositoryProjectList`/`OrganizationProjectList` if `< 3.17` |
   | `--assignee` | **no query** (logins sent as-is) | `RepositoryAssignableUsers` (all pages) |
5. **Create** (create.go:482, api/queries_issue.go:326-364), string mutation:
   ```graphql

	mutation IssueCreate($input: CreateIssueInput!) {
		createIssue(input: $input) {
			issue {
				id
				url
			}
		}
	}
   ```
   Variables `{"input": {...}}` where input contains:
   - always: `repositoryId`, `title`, `body` (body may be `""`);
   - `issueTemplate` (template name) only when a template was chosen (interactive/editor);
   - only when metadata flags were given (step 4): `labelIds` (array, or JSON `null` if no labels),
     `projectIds` (classic IDs; `null` when none — always `null` on github.com path),
     `milestoneId` (only if `--milestone`), and on GHES `assigneeIds` (array or `null`).
   On the github.com path `assigneeIds` is **never** sent (asserted by tests, create_test.go:717-718).
   Reads `data.createIssue.issue.id` and `.url`.
6. **Assign actors** (github.com path only, if ≥1 assignee; api/queries_issue.go:366-372, api/queries_pr.go:619-650):
   ```graphql
   mutation ReplaceActorsForAssignable($input:ReplaceActorsForAssignableInput!){replaceActorsForAssignable(input: $input){__typename}}
   ```
   Variables `{"input":{"assignableId":"<new issue id>","actorLogins":["monalisa","copilot-swe-agent[bot]"]}}`
   (login `copilot-swe-agent` gets the `[bot]` suffix, :630-632). Unknown logins should yield an error.
7. **Projects v2** (if any resolved; api/queries_issue.go:374-386): B5 with adds only.
8. **Deferred** (create.go:487-494, 509-555): `--type` → `RepositoryIssueTypes` (unless chosen interactively);
   `--parent`, `--blocked-by`, `--blocking` → one `IssueNodeID` each (sequential); then B6 mutations
   concurrently: `updateIssueIssueType`, `addSubIssue{issueId: parent, subIssueId: new, replaceParent: false}`,
   `addBlockedBy`. No feature check is made for relationships on create.
9. Prints `url` to stdout.

Not in the non-interactive path: interactive mode additionally may call `RepositoryIssueTypes` for a type prompt (create.go:363-378),
`SearchRepoAssignableActors` (`suggestedActors(first: 10, query: $query, capabilities: CAN_BE_ASSIGNED)` + `assignableUsers{totalCount}`, api/queries_repo.go:1355-1413) when ApiActorsSupported,
and template queries. `--web` only builds a URL (`/OWNER/REPO/issues/new?title=&body=&assignees=&labels=&projects=&milestone=`), resolving `--project` via B4 projects queries (params.go:16-52).
`--attach` uploads use a separate upload API (not covered here).

**Server-simplest path:** github.localhost: 1 `IssueRepositoryInfo`, optional label/milestone/project lookups, `createIssue`, optional `replaceActorsForAssignable`. The server must accept `null` for `labelIds`/`projectIds`/`assigneeIds` in `CreateIssueInput`.

---

### `gh issue list`

Source: pkg/cmd/issue/list/list.go, pkg/cmd/issue/list/http.go. Defaults: `--state open`, `--limit 30`.

Default field list (list.go:125-132,154): `number,title,url,state,updatedAt,labels,stateReason` →
fragment body `number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount},stateReason`.
With `--json F1,F2` the fragment is `IssueGraphQL([F1,F2])` instead (list.go:197-199; B7).

State handling (list.go:145-148): `--state open` becomes "no state" if `--search` contains any of
`is:closed`, `is:merged`, `state:closed`, `state:merged`, `merged:*`, `closed:*`.

Feature detector is constructed always but only queried in the search path or `--web`.

**Path selection** (list.go:232-263): if any of `--search`, `--label`, `--milestone`, `--type` → **search path**; otherwise **repository.issues path**.

#### A. repository.issues path (`--assignee`, `--author`/`--app`, `--mention`, `--state`, `--limit` only)

1. `@me` in `--assignee/--author/--mention` → `UserCurrent` (B3), once.
2. String query (http.go:28-45):
   ```graphql
   fragment issue on Issue {<FIELDS>}
	query IssueList($owner: String!, $repo: String!, $limit: Int, $endCursor: String, $states: [IssueState!] = OPEN, $assignee: String, $author: String, $mention: String) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issues(first: $limit, after: $endCursor, orderBy: {field: CREATED_AT, direction: DESC}, states: $states, filterBy: {assignee: $assignee, createdBy: $author, mentioned: $mention}) {
				totalCount
				nodes {
					...issue
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}
	
   ```
   (text begins `fragment issue on Issue {` with no leading newline; trailing `\n\t`.)
   Variables: `owner`, `repo`, `states` (`["OPEN"]`, `["CLOSED"]` or `["OPEN","CLOSED"]` for `all`),
   `limit` = `min(limit,100)`; `assignee`/`author`/`mention` only if set (otherwise absent ⇒ null; server must treat null filterBy members as "no filter").
   `--app foo` sends `author: "app/foo"`.
3. Pagination: while `hasNextPage` and fewer than `limit` collected: set `endCursor`, `limit = min(pageLimit, remaining)`. Reads `hasIssuesEnabled` (false ⇒ error), `issues.totalCount`, `nodes`.

#### B. search path

1. `--milestone` that parses as an integer → (list.go:265-287)
   ```graphql
   query RepositoryMilestoneByNumber($name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){milestone(number: $number){id,title}}}
   ```
   and the title replaces the number. `milestone: null` ⇒ error.
2. Reject if `--search` matches `(?i)\b(?:is|type):(?:pr|pull-?request)\b` (http.go:13,120-122).
3. `SearchFeatures()` — github.localhost: `SearchType_enumValues` introspection; GHES: `GET /api/v3/meta` (+ introspection if ≥3.18).
4. String query (http.go:133-147):
   ```graphql
   fragment issue on Issue {<FIELDS>}query IssueSearch($repo: String!, $owner: String!, $type: SearchType!, $limit: Int, $after: String, $query: String!) {
			repository(name: $repo, owner: $owner) {
				hasIssuesEnabled
			}
			search(type: $type, last: $limit, after: $after, query: $query) {
				issueCount
				nodes { ...issue }
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
   ```
   Note **`last: $limit`** (not `first`). Variables: `owner`, `repo`, `limit=min(limit,100)`, `query`, `type`:
   - `AdvancedIssueSearchAPI && ISSUE_ADVANCED in enum` ⇒ `"ISSUE_ADVANCED"`;
   - `AdvancedIssueSearchAPI` without it ⇒ `"ISSUE"` (advanced syntax);
   - GHES < 3.18 ⇒ `"ISSUE"` with legacy syntax.
   Pagination (http.go:190-216): sets `after = endCursor` and a **`perPage` variable that the query never declares** (limit is not reduced) — servers must ignore undeclared variables. Reads `repository.hasIssuesEnabled`, `search.issueCount` (as total), `search.nodes` (only `Issue` nodes get fields via the fragment), `pageInfo`. A server should treat `last:N` with no `before` as "first N results in relevance/default order" (that is how gh uses it).
5. Query string construction (pkg/cmd/pr/shared/params.go:235-267, pkg/search/query.go): qualifiers sorted by key, each `key:value`
   (value quoted with Go `%q` if it contains space, `"`, tab, CR, LF). Keys used: `assignee`, `author`, `label` (one per label, sorted),
   `mentions`, `milestone`, `repo` (`OWNER/REPO`), `state` (`open`/`closed`, omitted for `all`), `type` (`issue` plus `--type` value, sorted).
   Advanced syntax: `repo` group is `repo:O/R` (single, OR-group only if several); keywords from `--search` are wrapped `( <search> ) <qualifiers>`.
   Example `gh issue list -l bug -S "crash"` ⇒ `( crash ) label:bug repo:o/r state:open type:issue`; legacy: `crash label:bug repo:o/r state:open type:issue`.
   `@me` is **not** expanded in the search path (sent as `assignee:@me`).

`--web` only: `SearchFeatures()` then opens `/OWNER/REPO/issues?q=<query>`.

---

### `gh issue view`

Source: pkg/cmd/issue/view/view.go, pkg/cmd/issue/view/http.go. Arg is a number, `#N`, or an issue/PR URL (URL host/owner/repo override base repo; lookup.go:56-97).

Field set (view.go:96-152):
- default: `number,url,state,createdAt,title,body,author,milestone,assignees,labels,reactionGroups,lastComment,stateReason,issueType,parent,subIssues,subIssuesSummary`
  + (`--comments`: add `comments`, remove `lastComment`)
  + `projectItems` + (`projectCards` if ProjectsV1 supported) + (`blockedBy`,`blocking` if `IssueRelationshipsSupported`) + `id`.
- `--json F...`: exactly `F` + `id` (no detection).
- `--web`: `url`,`id`.

Requests:
1. Default/`--comments` only: `ProjectsV1()` and `IssueFeatures()` (github.localhost: none; GHES: `GET meta` + `Issue_fields`; an IssueFeatures error is tolerated and just omits blockedBy/blocking).
2. B1 `IssueByNumber`. Exact default text on github.localhost:
   ```graphql

	query IssueByNumber($owner: String!, $repo: String!, $number: Int!) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issue: issueOrPullRequest(number: $number) {
				__typename
				...on Issue{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount},stateReason,issueType{id,name,description,color},parent{id,number,title,url,state,repository{nameWithOwner}},subIssues(first:100){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},subIssuesSummary{total,completed,percentCompleted},blockedBy(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},blocking(first:50){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},id}
				...on PullRequest{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount},id}
			}
		}
	}
   ```
   With `--comments`, `comments(last: 1)…` is dropped from that position and `comments(first: 100) {…}` (B7 `comments`) is inserted right after `subIssuesSummary{…}` (before `blockedBy`); same for the PR fragment (after `reactionGroups`).
   On GHES without relationships, `blockedBy`/`blocking` are absent; on GHES < 3.17 `projectCards(first:100){…}` is appended after them (before `id`).
3. B2 `IssueProjectItems` (default and `--comments`, or `--json projectItems`).
4. If `comments` requested and `comments.pageInfo.hasNextPage` (view.go:157-162, http.go:11-54):
   ```graphql
   query CommentsForIssue($endCursor:String!$id:ID!){node(id: $id){...on Issue{comments(first: 100, after: $endCursor){nodes{id,author{login},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},totalCount,pageInfo{hasNextPage,endCursor}}},...on PullRequest{comments(first: 100, after: $endCursor){nodes{id,author{login},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},totalCount,pageInfo{hasNextPage,endCursor}}}}}
   ```
   loop until `hasNextPage=false`. (Note: page 2+ authors are `author{login}` only.)
5. If `closedByPullRequestsReferences` requested (only via `--json`) and `hasNextPage`:
   ```graphql
   query closedByPullRequestsReferences($endCursor:String!$id:ID!){node(id: $id){...on Issue{closedByPullRequestsReferences(first: 100, after: $endCursor){nodes{id,number,url,repository{id,name,owner{id,login}}},pageInfo{hasNextPage,endCursor}}}}}
   ```
Default output reads: `title, number, state, stateReason (NOT_PLANNED ⇒ gray), issueType.name, author.login/name, createdAt, comments.totalCount, reactionGroups, assignees.nodes[login,name]+totalCount, labels.nodes[name,color], parent, blockedBy/blocking nodes, projectItems (project.title, status.name) / projectCards, milestone.title, body, subIssuesSummary, subIssues.nodes, lastComment nodes, url`.

---

### `gh issue comment`

Source: pkg/cmd/issue/comment/comment.go, pkg/cmd/pr/shared/commentable.go, api/queries_comments.go. No feature detection.

1. B1 with fields `id,url` (comment.go:87-95); `--edit-last`/`--delete-last` add `comments` (first 100 only, **no pagination** — the "last own comment" is the last node with `viewerDidAuthor:true` among the first 100); `--attach` adds `repository`.
   ```graphql
   ...on Issue{id,url}
   ...on PullRequest{id,url}
   ```
2. Create (commentable.go:219-275, queries_comments.go:56-80):
   ```graphql
   mutation CommentCreate($input:AddCommentInput!){addComment(input: $input){commentEdge{node{url}}}}
   ```
   Variables `{"input":{"subjectId":"<issue/PR node id>","body":"..."}}`. Prints `commentEdge.node.url`.
3. `--edit-last` (commentable.go:277-346, queries_comments.go:82-104):
   ```graphql
   mutation CommentUpdate($input:UpdateIssueCommentInput!){updateIssueComment(input: $input){issueComment{url}}}
   ```
   Variables `{"input":{"id":"<comment id>","body":"..."}}`. With no own comment: error, or with `--create-if-none` falls back to step 2.
4. `--delete-last --yes` (queries_comments.go:106-125):
   ```graphql
   mutation CommentDelete($input:DeleteIssueCommentInput!){deleteIssueComment(input: $input){clientMutationId}}
   ```
   Variables `{"input":{"id":"<comment id>"}}`.
`--web` makes no write request (opens `<url>#issuecomment-new`).

---

### `gh issue close`

Source: pkg/cmd/issue/close/close.go. No feature detection (even for `--reason`/`--duplicate-of`).

1. B1 with `id,number,title,state` (close.go:112):
   `...on Issue{id,number,title,state}` / `...on PullRequest{id,number,title,state}`.
   If `state == "CLOSED"` → prints warning, exit 0, no further requests.
2. `--duplicate-of REF`: B1 on the referenced repo/number with fields `id` only; must be `__typename: Issue` (close.go:122-146).
3. `--comment TEXT`: `CommentCreate` (as in `issue comment`) **before** closing (close.go:148-162).
4. Close (close.go:174-214):
   ```graphql
   mutation IssueClose($input:CloseIssueInput!){closeIssue(input: $input){issue{id}}}
   ```
   Variables `{"input":{"issueId":"...","stateReason":"COMPLETED|NOT_PLANNED|DUPLICATE","duplicateIssueId":"..."}}`;
   `stateReason` omitted when `--reason` not given (`omitempty`); `--reason completed` ⇒ `COMPLETED`, `"not planned"` ⇒ `NOT_PLANNED`, `duplicate` (or `--duplicate-of` alone) ⇒ `DUPLICATE`; `duplicateIssueId` omitted unless `--duplicate-of`.
   If the number is a PR: `mutation PullRequestClose($input:ClosePullRequestInput!){closePullRequest(input: $input){pullRequest{id}}}` with `{"input":{"pullRequestId":"..."}}` (api/queries_pr.go:767-784).

---

### `gh issue reopen`

Source: pkg/cmd/issue/reopen/reopen.go.

1. B1 with `id,number,title,state` (reopen.go:82). `state == "OPEN"` ⇒ warning, exit 0.
2. `--comment`: `CommentCreate` first (reopen.go:92-105).
3. (reopen.go:118-138)
   ```graphql
   mutation IssueReopen($input:ReopenIssueInput!){reopenIssue(input: $input){issue{id}}}
   ```
   Variables `{"input":{"issueId":"..."}}`. PR: `mutation PullRequestReopen($input:ReopenPullRequestInput!){reopenPullRequest(input: $input){pullRequest{id}}}`.

---

### `gh issue edit`

Source: pkg/cmd/issue/edit/edit.go, pkg/cmd/pr/shared/editable.go, editable_http.go. Accepts multiple issue numbers/URLs (same repo); each is processed concurrently.

1. `IssueFeatures()` (edit.go:318) — github.localhost: none; GHES: `Issue_fields`.
2. If `--add-project/--remove-project`: `ProjectsV1()` (edit.go:339). **Additionally `ProjectsV1()` is always called at edit.go:392**, so on GHES `GET /api/v3/meta` happens on every edit (possibly cache-served).
3. B1 per issue (concurrent), fields (edit.go:323-357): `id,number,title,body,url` +
   - `--add/remove-assignee`: github.com path `assignedActors`; GHES `assignees`;
   - `--add/remove-label`: `labels`;
   - `--add/remove-project`: (`projectCards` if GHES<3.17) + `projectItems` (⇒ B2 per issue);
   - `--milestone`/`--remove-milestone`: `milestone`;
   - `--type`: `issueType`; `--parent`/`--remove-parent`: `parent`; `--attach`: `repository`.
   Example (github.localhost, assignee+label+milestone):
   `...on Issue{id,number,title,body,url,assignedActors(first: 10) {nodes {...on User {id,login,name,__typename}...on Bot {id,login,__typename}},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn}}` (PR fragment identical).
4. `FetchOptions` → B4 (editable.go:517-626), inputs:
   - labels if labels edited (`RepositoryLabelList`);
   - milestones (OPEN) if `--milestone` **or `--remove-milestone`** (fetched even when removing);
   - projects v2 (+classic on GHES<3.17) if projects edited;
   - assignable users: GHES only, when add/remove assignee given; github.com path: none;
   - `RepositoryIssueTypes` if `--type`.
   With only `--title`/`--body`, no metadata request is made.
5. Per issue `UpdateIssue` (editable_http.go:12-92) — concurrently:
   - `--add-label` (resolved IDs, case-insensitive):
     `mutation LabelAdd($input:AddLabelsToLabelableInput!){addLabelsToLabelable(input: $input){__typename}}` with `{"input":{"labelableId":"<issue id>","labelIds":["..."]}}`;
   - `--remove-label`:
     `mutation LabelRemove($input:RemoveLabelsFromLabelableInput!){removeLabelsFromLabelable(input: $input){__typename}}` same shape;
   - projects: B5 (adds `contentId=issue id`; removes use the project item `id` from B2);
   - if any of title/body/assignees/projects/milestone edited (one goroutine, sequential):
     a. github.com path & assignees edited: final login set = current `assignedActors` logins + adds − removes (`@me`→viewer login via B3, `@copilot`→`copilot-swe-agent`), then `ReplaceActorsForAssignable` (`actorLogins` = full final list, `copilot-swe-agent` ⇒ `copilot-swe-agent[bot]`);
     b. always then:
     ```graphql
     mutation IssueUpdate($input:UpdateIssueInput!){updateIssue(input: $input){__typename}}
     ```
     `input` (githubv4.UpdateIssueInput, omitempty pointers): `id` always; `title` iff `--title`; `body` iff `--body/--body-file`;
     `assigneeIds` (GHES path only, full final ID list); `projectIds` iff projects edited (classic IDs; **`[]` on github.com**);
     `milestoneId` iff milestone edited (`null` for `--remove-milestone`).
     On the github.com path an assignee-only edit sends `{"input":{"id":"..."}}` (a no-op `updateIssue`) — the server must accept it.
   For a PR number: `mutation PullRequestUpdate($input:UpdatePullRequestInput!){updatePullRequest(input: $input){__typename}}` instead.
6. Deferred (edit.go:572-639, B6): `IssueNodeID` lookups for `--parent/--add-sub-issue/--remove-sub-issue/--add-blocked-by/--remove-blocked-by/--add-blocking/--remove-blocking`, then concurrent mutations; `--parent` uses `replaceParent: true`; `--remove-parent` uses `removeSubIssue{issueId: <current parent.id>, subIssueId: this}`; `--type` ⇒ `updateIssueIssueType{issueTypeId}`, `--remove-type` ⇒ `issueTypeId: null`.
7. Prints sorted URLs of edited issues.

---

### `gh issue status`

Source: pkg/cmd/issue/status/status.go, api/queries_issue.go:396-473. No feature detection.

1. B3 `UserCurrent` (status.go:75).
2. String query; fields default `number,title,url,state,updatedAt,labels` (status.go:54-61) or `--json` fields:
   ```graphql
   fragment issue on Issue{number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount}}
	query IssueStatus($owner: String!, $repo: String!, $viewer: String!, $per_page: Int = 10) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			assigned: issues(filterBy: {assignee: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
			mentioned: issues(filterBy: {mentioned: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
			authored: issues(filterBy: {createdBy: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
		}
	}
   ```
   (no space before `{` in `on Issue{`, unlike `issue list`.) Variables `{"owner","repo","viewer":"<login>"}`; `$per_page` defaults to 10 (server must honour variable defaults). Note `filterBy.states` (inside `IssueFilters`) is used, not the top-level `states` arg. No pagination. Reads `hasIssuesEnabled` (false ⇒ error), the three `totalCount`s and `nodes`. `--json` output is `{"assigned":[...],"mentioned":[...],"createdBy":[...]}`.

---

### Summary of request counts (github.localhost, non-interactive, no cache effects)

| Command | Requests |
|---|---|
| `issue create -t T -b B` | IssueRepositoryInfo, IssueCreate |
| `+ -l x -m y -a u -p P` | + RepositoryLabelList, RepositoryMilestoneList, 3× ProjectV2List (concurrent) ; + ReplaceActorsForAssignable ; + UpdateProjectV2Items |
| `issue list` | IssueList (pages of ≤100) |
| `issue list -l bug` | SearchType_enumValues, IssueSearch (pages) |
| `issue view N` | IssueByNumber, IssueProjectItems (+CommentsForIssue pages with `--comments`) |
| `issue comment N -b x` | IssueByNumber, CommentCreate |
| `issue close N -c x -r "not planned"` | IssueByNumber, CommentCreate, IssueClose |
| `issue reopen N` | IssueByNumber, IssueReopen |
| `issue edit N -t x --add-label l` | IssueByNumber, RepositoryLabelList, LabelAdd ∥ IssueUpdate |
| `issue status` | UserCurrent, IssueStatus |

On GHES add: `GET /api/v3/meta` (create/view/edit/list-search), `Issue_fields` (create/view/edit), `RepositoryAssignableUsers` (create/edit with assignees), and `assigneeIds` replaces the actor mutation.

## 3.3 Pull request commands (create/list/view/checkout/diff/status) and PR field builder

All paths are relative to the `cli` repo root (gh v2.102.0). "GQL" means `POST` to the GraphQL endpoint
(`http://api.github.localhost/graphql` for `github.localhost`, `https://HOST/api/graphql` for GHES).
"REST" paths are relative to the REST base (`http://api.github.localhost/` or `https://HOST/api/v3/`).

Common request facts used below (details in the transport section):

* Every GQL request carries `GraphQL-Features: merge_queue` and `X-GitHub-Api-Version: 2022-11-28`
  (api/client.go:19-24,108-155). The body is `{"query": <text>, "variables": <object|null>}`
  (go-gh pkg/api/graphql_client.go:69). Go's `encoding/json` sorts map keys.
* Requests issued through `api.NewCachedHTTPClient(httpClient, 24h)` (feature detection) add the request
  header `X-GH-CACHE-TTL: 24h0m0s` (api/http_client.go:143-151). go-gh caches GET/HEAD and `POST /graphql`
  or `POST /api/graphql` responses with status <500 and !=403 on disk for 24h, keyed by method+URL+Accept+
  Authorization+body (go-gh pkg/api/cache.go:39-60). So **introspection queries are usually only sent
  once per 24h per token**; a server should still answer them every time.
* **String queries vs struct queries.** Some queries are hand-written strings (sent as written, including
  tabs and newlines). Others are Go structs that `shurcooL-graphql` (cli fork v0.0.4) turns into query text.
  Struct rendering works like this. Each field name is the Go field name in lowerCamelCase (`URL`→`url`,
  `ID`→`id`, `OID`→`oid`, `DatabaseID`→`databaseId`), unless a `graphql:"..."` tag gives the text to use.
  Sibling fields are joined by `,` with no spaces, and sub-selections use `{...}`. Variable declarations
  are `($a:T!$b:T2)`: sorted by name, with no separator. A Go pointer type gives a nullable type
  (`(*githubv4.String)(nil)` → `String`), and other types get `!`. The type name is the Go type name, so
  `githubv4.String`→`String`, `githubv4.ID`→`ID`, `githubv4.Int`→`Int`, and a locally declared struct such
  as `RequestReviewsByLoginInput` keeps its own name. A plain Go `string` is declared as `ID!` (HACK in
  shurcooL `query.go:82-84`), but no struct query in this section passes a plain string. A struct type that
  implements `json.Unmarshaler`, such as `time.Time`, is treated as a scalar and is not expanded. The
  operation is `query <Name>` or `mutation <Name>`. The struct-derived texts below were reconstructed using
  these rules, checked against the cloned cli/shurcooL-graphql v0.0.4. Servers should parse them as normal
  GraphQL, not compare them as strings.
* `shortenQuery` (api/query_builder.go:10-21) removes only `\n` and `\t`. It keeps spaces. This can
  produce sequences such as `__typename...on StatusContext {`, which is still valid GraphQL.

---

### PullRequestGraphQL field builder (api/query_builder.go)

`PullRequestGraphQL(fields)` (api/query_builder.go:469-474) works in three steps. It deduplicates
`fields` while keeping first-seen order (pkg/set is insertion-ordered). It then removes `issueOnlyFields`
(`isPinned, stateReason, closedByPullRequestsReferences, issueType, parent, subIssues, subIssuesSummary,
blockedBy, blocking`; :339-349). Finally it calls `IssueGraphQL`, which maps each name to a selection and
joins them with `,` (:385-465). Any name not in the table is emitted verbatim as a scalar field (`default:`
:460-461). `PullRequestFields`, the set allowed by `--json`, is defined at :353-382.

| field name | emitted GraphQL selection (exact text) |
|---|---|
| `author` | `author{login,...on User{id,name}}` |
| `mergedBy` | `mergedBy{login,...on User{id,name}}` |
| `headRepositoryOwner` | `headRepositoryOwner{id,login,...on User{name}}` |
| `headRepository` | `headRepository{id,name,nameWithOwner}` |
| `assignees` | `assignees(first:100){nodes{id,login,name,databaseId},totalCount}` |
| `assignedActors` (not in PullRequestFields) | `assignedActors(first: 10) {nodes {...on User {id,login,name,__typename}...on Bot {id,login,__typename}},totalCount}` |
| `labels` | `labels(first:100){nodes{id,name,description,color},totalCount}` |
| `projectCards` | `projectCards(first:100){nodes{project{name}column{name}},totalCount}` |
| `projectItems` | `projectItems(first:100){nodes{id, project{id,title}, status:fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},totalCount}` |
| `milestone` | `milestone{number,title,description,dueOn}` |
| `repository` (pseudo) | `repository{id,name,nameWithOwner,databaseId,viewerPermission}` |
| `reactionGroups` | `reactionGroups{content,users{totalCount}}` |
| `mergeCommit` | `mergeCommit{oid}` |
| `potentialMergeCommit` | `potentialMergeCommit{oid}` |
| `autoMergeRequest` | `autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}}` |
| `comments` | `comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount}` |
| `lastComment` (pseudo) | `comments(last: 1) {nodes {author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}}},totalCount}` |
| `reviewRequests` | `reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}}` |
| `reviews` | `reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount}` |
| `latestReviews` | `latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}}` |
| `files` | `files(first: 100) {nodes {additions,deletions,path,changeType}}` |
| `commits` | `commits(first: 100) {nodes {commit {authors(first:100) {nodes {name,email,user{id,login}}},messageHeadline,messageBody,oid,committedDate,authoredDate}}}` |
| `lastCommit` (pseudo) | `commits(last:1){nodes{commit{oid}}}` |
| `commitsCount` (pseudo) | `commits{totalCount}` |
| `requiresStrictStatusChecks` (pseudo) | `baseRef{branchProtectionRule{requiresStrictStatusChecks}}` |
| `statusCheckRollup` | `statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}}` |
| `statusCheckRollupWithCountByState` (pseudo) | `statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts {checkRunCount,checkRunCountsByState {state,count},statusContextCount,statusContextCountsByState {state,count}}}}}}` |
| `closingIssuesReferences` | `closingIssuesReferences(first: 100) {nodes {id,number,url,repository {id,name,owner {id,login}}}pageInfo{hasNextPage,endCursor}}` |
| everything else (`id, number, title, state, body, url, closed, createdAt, closedAt, updatedAt, additions, deletions, changedFiles, baseRefName, baseRefOid, headRefName, headRefOid, isCrossRepository, isDraft, maintainerCanModify, mergeable, mergedAt, mergeStateStatus, reviewDecision, fullDatabaseId, isInMergeQueue, isMergeQueueEnabled`) | the bare name |

Notes:

* `statusCheckRollup` comes from `StatusCheckRollupGraphQLWithoutCountByState("")` (:232-268). With an
  `after` value the text becomes `contexts(first:100,after:<after>)`; the finder passes the literal
  `$endCursor`. The counts variant is `StatusCheckRollupGraphQLWithCountByState()` (:208-230), and it is
  used only by `pr status`. `RequiredStatusCheckRollupGraphQL` (:270-312) adds
  `isRequired(pullRequestId: ...)` and an optional `event,`. It is used by `pr checks`, not by the
  commands in this section.
* `reviewRequests` always includes `...on Bot{login}`. According to the comment (:97-99), GHES "silently
  ignores" it. A strict GraphQL validator would reject a spread of `Bot` into a union that doesn't contain
  it, so **include `Bot` in `RequestedReviewer`**.
* `headRepositoryOwner` uses `...on User{name}`, so it must be an interface or union that includes `User`
  (`RepositoryOwner`).
* `PullRequest.statusCheckRollup` is the result of an aliased `commits(last: 1)` connection. The JSON key
  `statusCheckRollup` decodes into `{nodes:[{commit:{statusCheckRollup:{contexts:{...}}}}]}`
  (api/queries_pr.go:83-112).
* `ChecksStatus()` (api/queries_pr.go:357-430) uses the counts only if **both**
  `checkRunCountsByState` and `statusContextCountsByState` are non-empty lists (:367). Otherwise it
  iterates `contexts.nodes`, which are absent in the counts variant, and reports 0 checks.
* JSON export (`--json`) post-processing is in api/export_pr.go:123-250. `statusCheckRollup` is
  flattened: a CheckRun becomes `{__typename,name,workflowName,status,conclusion,startedAt,completedAt,
  detailsUrl}` and a StatusContext becomes `{__typename,context,state,targetUrl,startedAt(=createdAt)}`.
  `commits` is flattened, with `authors[{name,email,id,login}]`. `reviewRequests` keeps only User
  (`{__typename,login}`) and Team (`{__typename,name,slug(org/slug)}`), and drops Bot. `projectItems`
  becomes `[{status:{optionId,name},title}]`. Connections become their `nodes` arrays. Any other field
  comes from the struct by case-insensitive name. `Author` is marshalled as
  `{is_bot:false,login,id,name}`, or `{is_bot:true,login:"app/<login>"}` when `id` is empty
  (api/queries_issue.go:283-307). This is why **`author{...on User{id}}` must return an id for users**.
  Bots must not match `...on User`, so their id is empty.

---

### PR finder (pkg/cmd/pr/shared/finder.go)

`Find(opts)` (finder.go:111-299). Inputs: `Selector`, `Fields`, `BaseBranch`, `States`, `Detector`.

1. **URL selector.** `ParseURL` (:306-329) accepts `http`/`https` URLs whose path matches
   `^/([^/]+)/([^/]+)/pull/(\d+)(.*$)`. The base repo is `owner/repo@<URL host>` and the PR number comes
   from the URL. No git commands run.
2. Otherwise the base repo is `factory.BaseRepo()`, the "smart" resolver (pkg/cmd/root/root.go:163-169;
   pkg/cmd/factory/default.go:153-177; context/context.go:61-109). `-R/--repo` or `GH_REPO` override it
   (pkg/cmdutil/repo_override.go:75-86). Resolution order:
   * a git remote with `remote.<name>.gh-resolved` set (`base` or `owner/repo`) → no HTTP;
   * non-TTY → the first remote (sort order upstream > github > origin > others) → no HTTP;
   * TTY → the **RepositoryNetwork** query (below). One distinct repo means use it; several mean an error
     ("please run `gh repo set-default`").
3. **Empty selector (current branch)** (:131-166). git runs `symbolic-ref --quiet HEAD`, then
   `config --get-regexp ^branch\.<b>\.(remote|merge|pushremote|gh-merge-base)$`.
   * If `branch.<b>.merge` matches `^refs/pull/(\d+)/head$`, that number is used and the lookup is by
     number.
   * Otherwise `ResolvePullRequestRefs` runs (find_refs_resolution.go:135-170 and 317-394):
     `git rev-parse --symbolic-full-name <b>@{push}`. If that resolves to `refs/remotes/<remote>/<branch>`,
     those values are used. If not: `git config push.default` (with `upstream`/`tracking`, the branch is
     `branch.<b>.merge` without `refs/heads/`; otherwise it is the local name), then remote =
     `branch.<b>.pushremote` → `git config remote.pushDefault` → `branch.<b>.remote`. A remote name maps
     to a repo through `git remote -v`. A remote URL is parsed directly.
   * If the head repo differs from the base repo (`ghrepo.IsSame`: case-insensitive owner/name/host),
     the qualified head ref is `<headOwner>:<branch>`. Otherwise it is `<branch>`.
4. **Non-URL selector** (:167-193). `#123` or `123` means a number, **unless `BaseBranch` is set** (as
   `pr create` sets it), in which case it is always a branch. Anything else is a branch: `owner:branch`
   or `branch` (`ParseQualifiedHeadRef`, find_refs_resolution.go:55-66). More than one `:` is an error.
5. **Feature detection inside the finder.**
   * Only when `fields` contains `isInMergeQueue` or `isMergeQueueEnabled` (:212-226) does it call
     `PullRequestFeatures()`: the two introspection queries, **always, even for non-enterprise hosts**.
     If `PullRequest.fields` has no `isInMergeQueue`, both fields are dropped. No command in this section
     requests them.
   * `projectItems` is removed from the main query and fetched separately (:228-232, 287-296).
   * `projectCards` (:237-246): `ProjectsV1()` returns Unsupported for non-enterprise hosts with **no
     HTTP**, which drops the field. On GHES it sends `GET meta` (REST) and reads `installed_version`
     (feature_detection.go:335-348,570-585). If the version is <3.17.0, `projectCards` stays in the query.
6. `id` and `number` are always added to the field set (:208-210).
7. **Lookup by number.** If `prNumber > 0`, `findByNumber` runs (:356-384):

```graphql
	query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
		repository(owner: $owner, name: $repo) {
			pullRequest(number: $pr_number) {<PullRequestGraphQL(fields)>}
		}
	}
```
   Variables: `{"owner","repo","pr_number":<int>}`. Response: `data.repository.pullRequest`. A null PR
   decodes as an empty struct. gh relies on the server returning a GraphQL error (type `NOT_FOUND`).

8. **Lookup by branch.** If there is a base repo and a branch name, `findForRefs` runs (:386-442). The
   fields are those requested plus `state, baseRefName, headRefName, isCrossRepository,
   headRepositoryOwner`:

```graphql
	query PullRequestForBranch($owner: String!, $repo: String!, $headRefName: String!, $states: [PullRequestState!]) {
		repository(owner: $owner, name: $repo) {
			pullRequests(headRefName: $headRefName, states: $states, first: 30, orderBy: { field: CREATED_AT, direction: DESC }) {
				nodes {<PullRequestGraphQL(fields)>}
			}
			defaultBranchRef { name }
		}
	}
```
   Variables: `{"owner","repo","headRefName":<branch WITHOUT owner>,"states":<opts.States or null>}`.
   `states` is `null` for view, checkout and diff, and `["OPEN"]` for create.

   Client-side filtering happens next. Nodes are stably sorted so OPEN comes first. The first node is
   chosen where `HeadLabel()` equals the qualified head ref. `HeadLabel()` is `headRepositoryOwner.login +
   ":" + headRefName` if `isCrossRepository`, else `headRefName` (api/queries_pr.go:309-314). The node
   must also satisfy (no BaseBranch, or `baseRefName == BaseBranch`) and (`state == OPEN`, or
   `defaultBranchRef.name != qualifiedHeadRef`). No match gives `NotFoundError` ("no pull requests found
   for branch"). **So the server must filter `pullRequests(headRefName:)` by the bare branch name across
   all head repos, and return correct `isCrossRepository` and `headRepositoryOwner.login`.**
9. **Follow-up queries** run in parallel (errgroup, :266-298). Each runs only if its field was requested:
   * `reviews`, only if `reviews.pageInfo.hasNextPage`. Struct query, repeated until `hasNextPage` is
     false:
     `query ReviewsForPullRequest($endCursor:String!$id:ID!){node(id: $id){...on PullRequest{reviews(first: 100, after: $endCursor){nodes{id,author{login},authorAssociation,body,submittedAt,includesCreatedEdit,reactionGroups{content,users{totalCount}},state,url,commit{oid}},pageInfo{hasNextPage,endCursor},totalCount}}}}`
     (:444-482; struct api/queries_pr_review.go:29-50).
   * `comments`, only if `comments.pageInfo.hasNextPage`:
     `query CommentsForPullRequest($endCursor:String!$id:ID!){node(id: $id){...on PullRequest{comments(first: 100, after: $endCursor){nodes{id,author{login},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},totalCount,pageInfo{hasNextPage,endCursor}}}}}`
     (:484-522; struct api/queries_comments.go:9-40).
   * `closingIssuesReferences`, if `hasNextPage`:
     `query closingIssuesReferences($endCursor:String!$id:ID!){node(id: $id){...on PullRequest{closingIssuesReferences(first: 100, after: $endCursor){nodes{id,number,url,repository{id,name,owner{id,login}}},pageInfo{hasNextPage,endCursor}}}}}`
     (:524-561).
   * `statusCheckRollup`, only if `nodes[0].commit.statusCheckRollup.contexts.pageInfo.hasNextPage`.
     This is a string query (:563-614):

```graphql
	query PullRequestStatusChecks($id: ID!, $endCursor: String!) {
		node(id: $id) {
			...on PullRequest {
				statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100,after:$endCursor) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}}
			}
		}
	}
```
   * `projectItems` uses `api.ProjectsV2ItemsForPullRequest` (api/queries_projects_v2.go:136-205). It
     repeats while `hasNextPage`, starting with `endCursor = null`. Errors whose message contains
     `field requires one of the following scopes: ['read:project']`, `Field 'projectItems' doesn't exist
     on type 'PullRequest'`, or `Resource not accessible by` (and similar) are ignored (:328-343):
     `query PullRequestProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){pullRequest(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}`

**RepositoryNetwork** (api/queries_repo.go:477-571) is sent when the smart base repo resolves in a TTY.
`repo_000`… are generated for up to 5 remotes, with owner and name Go-`%q` quoted:

```graphql
	fragment repo on Repository {
		id
		databaseId
		name
		owner { login }
		viewerPermission
		defaultBranchRef {
			name
		}
		isPrivate
	}
	query RepositoryNetwork {
		viewer { login }
		repo_000: repository(owner: "OWNER", name: "REPO") {
			...repo
			parent {
				...repo
			}
		}
	}
```
`variables: null`. Errors that are all of `type: "NOT_FOUND"` are tolerated, and the matching keys are
`null`.

---

### `gh pr create`

Source: pkg/cmd/pr/create/create.go. Flags are at :386-410: `-B/--base`, `-H/--head`, `-f/--fill`,
`--fill-first`, `--fill-verbose`, `-t`, `-b`, `-F`, `-d/--draft`, `-r/--reviewer`, `-a/--assignee`,
`-l/--label`, `-p/--project`, `-m/--milestone`, `--no-maintainer-edit`, `-T/--template`, `--dry-run`,
`-w/--web`, `-e/--editor`, `--recover`, `--attach`.

When not interactive you must supply `--title` and `--body`, or one of `--fill`, `--fill-first`,
`--fill-verbose` (:349-351). `-R/--repo` or `GH_REPO` set `RepoOverride`; with an override, git remote
errors are ignored (:283-288, 1085-1096).

#### Step 1: NewCreateContext (:774-1083)

1. git: `remote -v` and `config --get-regexp ^remote\..*\.gh-resolved$` (git/client.go:165-194). These
   are filtered to authenticated hosts.
2. Base repo: `ResolveRemotesToRepos(remotes, client, RepoOverride)` then `.BaseRepo(io)`
   (context/context.go:19-109).
   * The result is a `*api.Repository` only if it came from the RepositoryNetwork query (TTY, nothing
     resolved). Then that object is used. **Its `databaseId` and `viewerPermission` are taken from the
     network query.**
   * Otherwise (override, `gh-resolved`, non-TTY first remote) gh sends **RepositoryInfo** (`api.GitHubRepo`,
     api/queries_repo.go:360-413):

```graphql
	fragment repo on Repository {
		id
		databaseId
		name
		owner { login }
		sshUrl
		hasIssuesEnabled
		description
		hasWikiEnabled
		viewerPermission
		defaultBranchRef {
			name
		}
	}

	query RepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			...repo
			parent {
				...repo
			}
			mergeCommitAllowed
			rebaseMergeAllowed
			squashMergeAllowed
		}
	}
```
   A null `repository` becomes a synthetic `NOT_FOUND` error. The fields read are `id`, which becomes
   `repositoryId`, plus `defaultBranchRef.name` and `viewerPermission`
   (ViewerCanTriage = ADMIN/MAINTAIN/WRITE/TRIAGE).
3. **The base branch** is `--base`, else `git config branch.<head>.gh-merge-base`, else
   `defaultBranchRef.name` (:853-859, 884-890). There is no extra request for the default branch.
4. **`--head` given** (:842-866). It is parsed as `owner:branch` or `branch`. git runs
   `config --get-regexp ^branch\.<branch>\.(remote|merge|pushremote|gh-merge-base)$`. The result is
   `skipPushRefs`: **no push**, and `headRefName` is sent as given (`owner:branch` is left to the server
   to resolve).
5. **No `--head`** (:868-1082):
   * git `status --porcelain`: a warning only.
   * git `symbolic-ref --quiet HEAD`.
   * branch config.
   * `TryDetermineDefaultPRHead`, the same @{push} / pushRemote / remote.pushDefault / remote algorithm
     as the finder.
   * If the head repo has a matching remote, git runs
     `show-ref --verify -- HEAD refs/remotes/<headRemote>/<pushBranch>`. If both hashes are equal, the
     result is `skipPushRefs` with head `<branch>` or `<headOwner>:<branch>`. The owner is compared as a
     raw string against the base owner.
   * Otherwise git runs `show-ref --verify -- HEAD refs/remotes/<r>/<currentBranch>` for every remote.
     The first one equal to HEAD gives `skipPushRefs`. (`--verify` stops at the first missing ref; this
     is a known bug kept as-is, :946-966.)
   * **No match and non-TTY**: it prints `aborted: you must first push the current branch to a remote, or
     use the --head flag` and exits. **Non-interactive `gh pr create` never pushes.**
   * **TTY**:
     * `HeadRepos()` reuses or sends RepositoryNetwork and keeps repos with WRITE/MAINTAIN/ADMIN.
     * If there are none, it sends **RepositoryFindFork** (api/queries_repo.go:696-741):
       ```graphql
       	query RepositoryFindFork($owner: String!, $repo: String!, $limit: Int!) {
       		repository(owner: $owner, name: $repo) {
       			forks(first: $limit, affiliations: [OWNER, COLLABORATOR]) {
       				nodes {
       					id
       					name
       					owner { login }
       					url
       					viewerPermission
       				}
       			}
       		}
       	}
       ```
       with `limit=3`.
     * Then **UserCurrent**: `query UserCurrent{viewer{login}}` (api/queries_user.go:7-15).
     * The user is prompted with: pushable repos / "Create a fork of O/R" / "Skip pushing the branch" /
       "Cancel".
6. `isSameRef`: if the head has no `:` and the head branch equals the base branch, it errors (:180-185,
   428-430).

#### Step 2: createRun (:422-705)

1. Detector (cached client):
   * `ProjectsV1()`: non-enterprise means no HTTP and Unsupported. GHES sends `GET meta` (REST).
   * `IssueFeatures()`: non-enterprise means no HTTP and `ApiActorsSupported=true`. GHES sends:
     `query Issue_fields{Issue: __type(name: "Issue"){fields(includeDeprecated: true){name}}}`, and
     ApiActorsSupported is always false (feature_detection.go:189-223).
   * `PullRequestFeatures()` is **not** called by `pr create`.
2. `NewIssueState` (:739-772):
   * `@me` in `--assignee` triggers UserCurrent (MeReplacer, pkg/cmd/pr/shared/params.go:291-327).
   * `@copilot` in an assignee becomes `copilot-swe-agent` (only when ApiActorsSupported). `@copilot` as a
     reviewer becomes `copilot-pull-request-reviewer`.
   * **git log for `--fill`** runs whenever any fill flag is set, or title or body is missing (the error is
     ignored unless a fill flag is set):
     `git -c log.ShowSignature=false log --pretty=format:%H%x00%s%x00%b%x00 --cherry <baseRemote>/<base>...<headBranch>`
     (git/client.go:367-410). The left side is `<remoteName>/<base>` if a remote for the base repo
     exists, else bare `<base>` (:810-831). With one commit (or `--fill-first`, which uses the oldest), the
     title and body come from that commit. Otherwise the title is the branch name with `-` and `_` replaced
     by spaces, and the body is a bullet list of commit titles (`--fill-verbose` adds the bodies)
     (:709-737).
3. `--web` (:485-506): handlePush, then the browser opens
   `<HostPrefix>/<owner>/<repo>/compare/<base>...<qualifiedHead>?expand=1&title=..&body=..&labels=..&assignees=..&projects=..&milestone=..`.
   Projects go through `ProjectTitlesToPaths` (api/queries_repo.go:875), which makes v2 project queries.
4. **Check for an existing PR** (:516-529). It uses `Finder.Find` with `Selector=<qualifiedHeadRef>`,
   `BaseBranch=<base>`, `States=["OPEN"]`, `Fields=["url"]`. The finder resolves the base repo again
   through the smart resolver, so a TTY may send a second RepositoryNetwork. The request is
   **PullRequestForBranch** with this selection:
   `url,id,number,state,baseRefName,headRefName,isCrossRepository,headRepositoryOwner{id,login,...on User{name}}`
   and variables `{"owner","repo","headRefName":"<branch w/o owner>","states":["OPEN"]}`.
   If it finds a PR, the error is: `a pull request for branch "<head>" into branch "<base>" already
   exists:\n<url>`.
5. Interactive only:
   * Templates through `NewTemplateManager` (pkg/cmd/pr/shared/templates.go:144-291). It calls
     `RepositoryFeatures()`. Non-enterprise means no HTTP and `PullRequestTemplateQuery=true`. GHES sends
     `query Repository_fields{Repository: __type(name: "Repository"){fields(includeDeprecated: true){name}}}`
     and needs `pullRequestTemplates` in the result.
   * If the API is supported:
     `query PullRequestTemplates($name:String!$owner:String!){repository(owner: $owner, name: $name){pullRequestTemplates{filename,body}}}`
     (:89-115). Templates on disk are used as well: the legacy template always, and non-legacy templates
     only without the API.
   * `issueTemplates` (`query IssueTemplates(...){repository(...){issueTemplates{name,body,title}}}`) is
     used only by issue create.
   * The metadata survey can make reviewer and assignee search queries (`SuggestedReviewerActorsForRepo`,
     `SearchRepoAssignableActors`); these are interactive only.
6. **handlePush** (:1243-1360). Only `pushableRefs` and `forkableRefs` push. From the TTY prompt:
   * Fork: `POST repos/{owner}/{repo}/forks` with body `{"default_branch_only":true}`. gh reads
     `node_id,name,owner.login,created_at`. If the fork equals the source repo, it errors
     (api/queries_repo.go:595-642).
   * If the head repo has no remote, gh adds one. It may first run `git remote rename origin upstream`,
     then `git remote add <origin|fork> <url>`. The URL is `http://github.localhost/OWNER/REPO.git` for
     `github.localhost`, else `https://HOST/OWNER/REPO.git` (or the ssh form)
     (internal/ghrepo/repo.go:95-103; internal/ghinstance/host.go:102-107). After a fork, gh also runs
     `git config --add remote.upstream.gh-resolved base`.
   * Push:
     `git -c credential.helper= -c credential.helper=!"gh" auth git-credential push --set-upstream <remote> HEAD:refs/heads/<branch>`
     (git/client.go:911-921). gh retries 3 times, 2s apart, only after a fork. Stderr lines matching
     `^remote: (Create a pull request.*by visiting|[[:space:]]*https://.*/pull/new/).*` are hidden
     (:1408). Smart HTTP sends `GET /<o>/<r>.git/info/refs?service=git-receive-pack` and then
     `POST /<o>/<r>.git/git-receive-pack`.
7. **submitPR** (:1098-1160):
   * Params: `title, body, draft, baseRefName, headRefName (qualified), maintainerCanModify`. A blank
     title is an error.
   * `AddMetadataToIssueParams` (pkg/cmd/pr/shared/params.go:59-169) runs only when there are reviewers,
     assignees, labels, projects or a milestone. It calls `api.RepoMetadata` (api/queries_repo.go:986-1100)
     with parallel queries:
     * labels:
       `query RepositoryLabelList($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){labels(first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name},pageInfo{hasNextPage,endCursor}}}}`
     * milestone:
       `query RepositoryMilestoneList($endCursor:String$name:String!$owner:String!$states:[MilestoneState!]!){repository(owner: $owner, name: $name){milestones(states: $states, first: 100, after: $endCursor){nodes{id,title},pageInfo{hasNextPage,endCursor}}}}`
       with `states=["OPEN"]`.
     * projects, three queries; `ProjectsV2IgnorableError` and `Could not resolve to an Organization`
       errors are ignored:
       `query UserProjectV2List($endCursor:String$query:String!){viewer{projectsV2(first: 100, orderBy: {field: TITLE, direction: ASC}, after: $endCursor, query: $query){nodes{id,title,number,resourcePath,closed,url},pageInfo{hasNextPage,endCursor}}}}`,
       `query RepositoryProjectV2List($endCursor:String$name:String!$owner:String!$query:String!){repository(owner: $owner, name: $name){projectsV2(...same...)}}`,
       `query OrganizationProjectV2List($endCursor:String$owner:String!$query:String!){organization(login: $owner){projectsV2(...same...)}}`.
       All use `query="is:open"`.
     * Projects v1 (GHES <3.17 only): `RepositoryProjectList` and `OrganizationProjectList`, using
       `projects(states: [OPEN], first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name,number,resourcePath},pageInfo{...}}`.
     * **GHES only** (`ApiActorsSupported=false`):
       * `query RepositoryAssignableUsers($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){assignableUsers(first: 100, after: $endCursor){nodes{id,login,name},pageInfo{hasNextPage,endCursor}}}}`
       * for `org/team` reviewers:
         `query OrganizationTeamList($endCursor:String$owner:String!){organization(login: $owner){teams(first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,slug},pageInfo{...}}}}`
       * plus UserCurrent.
     * Name matching for labels, milestones and projects is case-insensitive.
   * `--dry-run` prints here and stops; metadata queries still run.
   * **Mutation** (api/queries_pr.go:490-523):

```graphql
		mutation PullRequestCreate($input: CreatePullRequestInput!) {
			createPullRequest(input: $input) {
				pullRequest {
					id
					url
				}
			}
	}
```
   Variables: `{"input":{"baseRefName","body","draft","headRefName","maintainerCanModify","repositoryId","title"}}`.
   `headRefName` is `branch` or `owner:branch`. **`draft` and `maintainerCanModify` are always present
   as booleans.** gh reads `createPullRequest.pullRequest.{id,url}` and prints the URL to stdout.

   Follow-up mutations run in order, each only if needed (:526-612):
   1. `assigneeIds`, `labelIds`, `projectIds` (v1), `milestoneId`, when non-blank:
      ```graphql
      		mutation PullRequestCreateMetadata($input: UpdatePullRequestInput!) {
      			updatePullRequest(input: $input) { clientMutationId }
      		}
      ```
      `input = {pullRequestId, labelIds?, milestoneId?, projectIds?, assigneeIds?}`. On
      `github.localhost` (ApiActorsSupported) `assigneeIds` is never set.
   2. Assignees by login (non-enterprise): `mutation ReplaceActorsForAssignable($input:ReplaceActorsForAssignableInput!){replaceActorsForAssignable(input: $input){__typename}}`
      with `input={"assignableId":<prId>,"actorLogins":[...]}`. `copilot-swe-agent` is sent as
      `copilot-swe-agent[bot]` (:619-650).
   3. Reviewers:
      * non-enterprise: `mutation RequestReviewsByLogin($input:RequestReviewsByLoginInput!){requestReviewsByLogin(input: $input){clientMutationId}}`
        with `input={"pullRequestId","userLogins":[..],"botLogins":[..],"teamSlugs":["org/slug"],"union":true}`.
        All three arrays are always present (possibly `[]`). Bot logins get the suffix `[bot]`
        (api/queries_pr_review.go:358-405).
      * GHES: `mutation PullRequestCreateRequestReviews($input: RequestReviewsInput!) { requestReviews(input: $input) { clientMutationId } }`
        with `{pullRequestId,userIds?,teamIds?,union:true}`.
   4. Projects v2, when there is at least one id (an empty map means no request):
      `mutation UpdateProjectV2Items($input_000: AddProjectV2ItemByIdInput!) {add_000: addProjectV2ItemById(input: $input_000) { item { id } }}`
      with `input_000={"contentId":<prId>,"projectId":<id>}` (api/queries_projects_v2.go:35-62).

   A failure after creation prints the URL and then the error `pull request update failed: ...`.
   `--attach` uploads go through internal/attachments; they are out of scope here.

**Minimal non-interactive sequence** (`gh pr create --fill` or `-t/-b`, non-TTY, `github.localhost`, branch
already pushed, no metadata):
1. RepositoryInfo.
2. PullRequestForBranch (`states:["OPEN"]`, expects no match).
3. PullRequestCreate.

There are no REST calls and no introspection.

---

### `gh pr list`

Source: pkg/cmd/pr/list/list.go and list/http.go. Flags: `-L/--limit` (default 30), `-s/--state`
(`open|closed|merged|all`), `-B/--base`, `-H/--head`, `-l/--label`, `-A/--author`, `--app`,
`-a/--assignee`, `-S/--search`, `-d/--draft`, `--json`, `-w/--web`.

* The base repo comes from the smart resolver (above). `--app X` becomes author `app/X`.
* If `--search` contains a state clause (`is:closed`, `is:merged`, `state:closed`, `state:merged`,
  `merged:*`, `closed:*`), the state becomes "" (:161-164).
* **Default fields** (:133-143): `number,title,state,url,headRefName,headRepositoryOwner{id,login,...on User{name}},isCrossRepository,isDraft,createdAt`.
  With `--json`, the fields are exactly those requested, passed through PullRequestGraphQL. **No
  feature detection and no follow-up pagination** happen, so `statusCheckRollup` gives only the first 100
  contexts, `projectItems` and `projectCards` stay inline, and `projectCards` gets no ProjectsV1 check.
* **Search path** is used when `--draft`, `--author`, `--assignee`, `--search`, or `--label` is set
  (http.go:13-15). Otherwise the repository path is used.

**Repository path** (pkg/cmd/pr/shared/lister.go:38-146):

```graphql
fragment pr on PullRequest{<fields>}
		query PullRequestList(
			$owner: String!,
			$repo: String!,
			$limit: Int!,
			$endCursor: String,
			$baseBranch: String,
			$headBranch: String,
			$state: [PullRequestState!] = OPEN
		) {
			repository(owner: $owner, name: $repo) {
				pullRequests(
					states: $state,
					baseRefName: $baseBranch,
					headRefName: $headBranch,
					first: $limit,
					after: $endCursor,
					orderBy: {field: CREATED_AT, direction: DESC}
				) {
					totalCount
					nodes {
						...pr
					}
					pageInfo {
						hasNextPage
						endCursor
					}
				}
			}
		}
```
Variables:
* `owner`, `repo`, `limit = min(limit,100)`.
* `state`: open→`["OPEN"]`, closed→`["CLOSED","MERGED"]`, merged→`["MERGED"]`, all→`["OPEN","CLOSED","MERGED"]`.
* `baseBranch` and `headBranch` are present only if set. `endCursor` is absent on the first page.

Pagination continues while `hasNextPage` and fewer than `limit` results have been collected. The next page
size is `min(pageLimit, limit-len)`. Duplicate `number`s are dropped. `totalCount` is used for the TTY
header.

**Search path** (http.go:32-130):

```graphql
fragment pr on PullRequest{<fields>}
		query PullRequestSearch(
			$q: String!,
			$type: SearchType!,
			$limit: Int!,
			$endCursor: String,
		) {
			search(query: $q, type: $type, first: $limit, after: $endCursor) {
				issueCount
				nodes {
					...pr
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
```
Before the search, gh calls `detector.SearchFeatures()` (feature_detection.go:399-...):
* Non-enterprise: no version check, then
  `query SearchType_enumValues{SearchType: __type(name: "SearchType"){enumValues(includeDeprecated: true){name}}}`.
* GHES: `GET meta` first. Only versions ≥3.18 send the enum query; older versions use legacy syntax with
  `type=ISSUE`.

If the enum contains `ISSUE_ADVANCED`, `type="ISSUE_ADVANCED"`; otherwise `"ISSUE"`. **Simplest: omit
`ISSUE_ADVANCED` from `SearchType` → `type: "ISSUE"`.** `q` uses advanced syntax
(pkg/search/query.go:153-173): qualifiers are sorted by key and joined with spaces, and keywords become
`( <keywords> ) <qualifiers>`. Examples:
* `--author @me --label bug` → `author:@me label:bug repo:OWNER/REPO state:open type:pr`.
* `--state merged` → `... is:merged ...`.
* `--draft` → `draft:true`.

Values containing spaces are `%q`-quoted. **`@me` is sent verbatim; the server must resolve it.** gh reads
`issueCount`, `nodes` and `pageInfo`. Results above 1000 trigger a warning.

`--web` opens `<HostPrefix>/OWNER/REPO/pulls?q=<legacy query>`, with no API call.

---

### `gh pr view [<number> | <url> | <branch>]`

Source: pkg/cmd/pr/view/view.go.
* `--web` uses fields `url` only, giving `url,id,number`.
* `--json` uses exactly the requested fields, plus `id,number`.
* `--comments` uses the same fields as the default view and only changes rendering.

**Default fields** (:89-95): url, number, title, state, body, author, autoMergeRequest, isDraft,
maintainerCanModify, mergeable, additions, deletions, commitsCount, baseRefName, headRefName,
headRepositoryOwner, headRepository, isCrossRepository, reviewRequests, reviews, assignees, labels,
projectCards, projectItems, milestone, comments, reactionGroups, createdAt, statusCheckRollup.

Request sequence:
1. Base repo resolution (smart resolver, or the URL).
2. `projectCards`: for non-enterprise it is dropped with no HTTP. For GHES, `GET meta` (REST) decides;
   the field is kept if the version is <3.17.
3. **PullRequestByNumber** (number or URL selector, or a branch whose `merge` is `refs/pull/N/head`) or
   **PullRequestForBranch** (branch or no argument; `states: null`) with this exact selection
   (non-enterprise):
   `url,number,title,state,body,author{login,...on User{id,name}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},isDraft,maintainerCanModify,mergeable,additions,deletions,commits{totalCount},baseRefName,headRefName,headRepositoryOwner{id,login,...on User{name}},headRepository{id,name,nameWithOwner},isCrossRepository,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},reactionGroups{content,users{totalCount}},createdAt,statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}},id`
   * The branch variant appends no new names, because the required fields are already present.
   * On GHES <3.17, `projectCards(first:100){nodes{project{name}column{name}},totalCount}` is inserted
     after `labels`.
4. Running in parallel, as needed: ReviewsForPullRequest and CommentsForPullRequest (only when
   `hasNextPage`), PullRequestStatusChecks (only when `hasNextPage`), and **PullRequestProjectItems
   (always, because projectItems is a default field)**.

There is no `PullRequestFeatures` introspection: the default fields have no merge-queue fields, and the
status checks use the non-count variant with no detection.

**Rendering (TTY)** (:187-298):
* Header: title, `owner/repo#number`, state (`isDraft`, `state`), author, `commits.totalCount`,
  `baseRefName`, `headRefName`, `createdAt`.
* `+additions -deletions`, a checks summary from contexts, reactions, reviewers (reviews whose author is
  not the PR author plus reviewRequests), assignees (with an ellipsis if totalCount > nodes), labels
  (`color`), projects (`projectItems.status.name`, or "No Status"), milestone title, auto-merge
  (`enabledBy.login`, `mergeMethod`), body as markdown.
* Comments and reviews appear if `totalCount > 0`. The preview shows the last comment.
* Footer: url.

**Non-TTY** (:146-185): `title, state (DRAFT if isDraft&&OPEN), author, labels, assignees, reviewers,
projects, milestone, number, url, additions, deletions, auto-merge`, then `--` and the body. With
`--comments` and no TTY, it prints the raw comment list.

---

### `gh pr checkout [<number> | <url> | <branch>]`

Source: pkg/cmd/pr/checkout/checkout.go. Flags: `-b/--branch`, `-f/--force`, `--detach`,
`--recurse-submodules`, `--worktree <path>`.

* **With an argument**: the finder runs with fields `number, headRefName, headRepository,
  headRepositoryOwner, isCrossRepository, maintainerCanModify` (:409-425). With `id` added the selection
  is:
  `number,headRefName,headRepository{id,name,nameWithOwner},headRepositoryOwner{id,login,...on User{name}},isCrossRepository,maintainerCanModify,id`.
  The branch variant adds `state,baseRefName`.
* **Without an argument, TTY**: smart BaseRepo, then PullRequestList (the lister query above) with
  `state=["OPEN"]`, `limit=10`, and fragment
  `number,title,state,isDraft,headRefName,headRepository{id,name,nameWithOwner},headRepositoryOwner{id,login,...on User{name}},isCrossRepository,maintainerCanModify`.
  The user then picks one (:436-478). Non-TTY without an argument is an error.

Remote selection (:127-170):
* The base remote is the git remote whose owner and name equal the base repo. Otherwise the base is a URL
  built with `git_protocol` from config: `http://github.localhost/OWNER/REPO.git`, `https://HOST/...`,
  or `git@HOST:OWNER/REPO.git`.
* `headRemote` is the base remote for same-repo PRs. For cross-repo PRs it is the remote matching
  `headRepositoryOwner.login`/`headRepository.name`. It is nil if `headRepository` is null (deleted
  fork).
* A `headRefName` starting with `-` is rejected.

**Existing head remote** (:199-242). Git commands, with the `fetch` authenticated via
`-c credential.helper= -c credential.helper=!"gh" auth git-credential`:
* `fetch <remote> +refs/heads/<headRefName>:refs/remotes/<remote>/<headRefName> --no-tags`
  (with `--detach`, the refspec is `+refs/heads/<headRefName>` and the checkout is
  `checkout --detach FETCH_HEAD`).
* Then either:
  * `checkout -b <local> --track <remote>/<headRefName>`, or
  * if a local branch exists (`rev-parse --verify refs/heads/<local>`): `checkout <local>`, then
    `merge --ff-only refs/remotes/<remote>/<head>` (`reset --hard` with `--force`).
* No HTTP beyond the finder.

**Missing head remote** (cross-repo with no remote, deleted fork, or base not a remote) (:244-312):
1. HTTP: `api.RepoDefaultBranch`. The finder returns a plain repo, not `*api.Repository`, so this sends
   **RepositoryInfo** (the full text above) to read `defaultBranchRef.name` (api/queries_repo.go:415-425).
2. The local branch name is `<headRefName>`. If that equals the default branch, it is
   `<headRepositoryOwner.login>/<headRefName>`. `--branch` overrides it.
3. git:
   * `fetch <baseURLOrName> refs/pull/<N>/head:<local> --no-tags [--force]`, then `checkout <local>`.
   * If `<local>` is the current branch: `fetch <base> refs/pull/<N>/head --no-tags`, then
     `merge --ff-only FETCH_HEAD`.
   * Worktree variants use `worktree add -- <path> <local>`, or `-C <path> fetch ...` /
     `checkout -b <local> FETCH_HEAD`.
4. If `git config branch.<local>.merge` is empty, git sets:
   * `branch.<local>.remote` and `branch.<local>.pushRemote` to `<baseURLOrName>`, or to the **head repo
     URL** (`http(s)://HOST/<headOwner>/<headRepo.name>.git`) when `maintainerCanModify && headRepository
     != null`;
   * `branch.<local>.merge` to `refs/pull/<N>/head`, or to `refs/heads/<headRefName>` in the
     maintainerCanModify case.

**Server git requirements**:
* The base repo's git endpoint must serve `refs/pull/<N>/head` for every PR, including cross-repo ones,
  for `git-upload-pack` (`GET /<o>/<r>.git/info/refs?service=git-upload-pack`, `POST
  .../git-upload-pack`, protocol v2 `ls-refs` with `ref-prefix refs/pull/N/head`).
* Head branches are fetched as `refs/heads/<headRefName>` from the head remote.
* `--recurse-submodules` adds `submodule sync --recursive` and `submodule update --init --recursive`.
  These are authenticated with the pattern `http://github.localhost` (git/client.go:135-141).

---

### `gh pr diff [<number> | <url> | <branch>]`

Source: pkg/cmd/pr/diff/diff.go.
1. Finder with fields `["number"]` (`--web`: `["url"]`), giving the selection `number,id`. The branch
   variant is `number,id,state,baseRefName,headRefName,isCrossRepository,headRepositoryOwner{id,login,...on User{name}}`.
2. **REST** (:212-236):
   `GET repos/{owner}/{repo}/pulls/{number}` with header `Accept: application/vnd.github.v3.diff`, or
   `Accept: application/vnd.github.v3.patch` with `--patch`.
   * `--name-only` forces diff mode. Owner and repo are path-escaped (`safeurl.JoinPath`).
   * The per-request Accept replaces go-gh's default `application/vnd.github.merge-info-preview+json,
     application/vnd.github.nebula-preview`. `X-GitHub-Api-Version: 2022-11-28` and auth are added.
   * **Any status other than 200 is an error** ("could not find pull request diff: ..."). The body is
     streamed raw.
3. Client side:
   * `--name-only` extracts names with `(?:^|\n)diff\s--git.*\s("?)b/(.*)` (:359), so the body must be a
     git-style unified diff with `diff --git a/X b/X` headers.
   * `--exclude` splits on `\ndiff --git `.
   * `--patch` expects `git format-patch` mbox output (`From <sha> Mon Sep 17 00:00:00 2001` …).
   * If output is not a TTY and the body contains terminal escape sequences, gh refuses to print it unless
     `--allow-escape-sequences` is passed.
4. `--web` opens `<pr.url>/files`, with no REST call.

---

### `gh pr status`

Source: pkg/cmd/pr/status/status.go and status/http.go. Flags: `-c/--conflict-status`, `--json`, `-R`.

1. Smart BaseRepo.
2. Without `-R`, the current branch is determined by git (`symbolic-ref`, branch config). If
   `branch.<b>.merge` is `refs/pull/N/head`, `CurrentPR=N`. Otherwise the qualified head ref
   (`<branch>` or `<owner>:<branch>`) is resolved with the same @{push} algorithm (:98-134).
3. **Always** calls `PullRequestFeatures()` with the cached client (:146-154, feature_detection.go:225-292).
   It sends two GQL requests in parallel:
   * `query PullRequest_fields{PullRequest: __type(name: "PullRequest"){fields(includeDeprecated: true){name}},StatusCheckRollupContextConnection: __type(name: "StatusCheckRollupContextConnection"){fields(includeDeprecated: true){name}}}`
   * `query PullRequest_fields2{WorkflowRun: __type(name: "WorkflowRun"){fields(includeDeprecated: true){name}}}`

   Only `CheckRunAndStatusContextCounts` is used, and it is true if
   `StatusCheckRollupContextConnection` has a field named `checkRunCount`. `isInMergeQueue`
   (MergeQueue) and `event` (CheckRunEvent) do not affect this command. If `__type` returns `null`, the
   feature is false.
4. Username: `@me`. **On non-enterprise hosts `@me` is sent verbatim inside the search strings.** On GHES
   it is first resolved with UserCurrent (http.go:189-198).
5. **Main query** (http.go:32-187). The default text is `<fragments> + <prefix> + <searches>`.
   The fragments (http.go:200-223) are:

```graphql
	fragment pr on PullRequest {<PR_FIELDS>}
	fragment prWithReviews on PullRequest {...pr,reviewDecision,latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}}}
```
   where `<PR_FIELDS>` is:
   * **counts supported** (`checkRunCount` exists):
     `number,title,state,url,isDraft,isCrossRepository,headRefName,headRepositoryOwner{id,login,...on User{name}},mergeStateStatus,baseRef{branchProtectionRule{requiresStrictStatusChecks}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts {checkRunCount,checkRunCountsByState {state,count},statusContextCount,statusContextCountsByState {state,count}}}}}}`
   * **not supported**: the same, but ending in
     `statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}}`
     (no pagination follows).
   * `--conflict-status` inserts `,mergeable` before `statusCheckRollup...`.
   * **Simplest server**: do *not* expose `checkRunCount` on `StatusCheckRollupContextConnection`. Then
     status uses the same `contexts(first:100){nodes...}` shape that view uses, and you avoid
     implementing the four count fields.

   The prefix depends on `CurrentPR`. With no PR number from git config:

```graphql
	query PullRequestStatus($owner: String!, $repo: String!, $headRefName: String!, $viewerQuery: String!, $reviewerQuery: String!, $per_page: Int = 10) {
		repository(owner: $owner, name: $repo) {
			defaultBranchRef {
				name
			}
			pullRequests(headRefName: $headRefName, first: $per_page, orderBy: { field: CREATED_AT, direction: DESC }) {
				totalCount
				edges {
					node {
						...prWithReviews
					}
				}
			}
		}
	
```
   With `CurrentPR > 0`:

```graphql
		query PullRequestStatus($owner: String!, $repo: String!, $number: Int!, $viewerQuery: String!, $reviewerQuery: String!, $per_page: Int = 10) {
			repository(owner: $owner, name: $repo) {
				defaultBranchRef {
					name
				}
				pullRequest(number: $number) {
					...prWithReviews
					baseRef {
						branchProtectionRule {
							requiredApprovingReviewCount
						}
					}
				}
			}
		
```
   Then, for both:

```graphql
			viewerCreated: search(query: $viewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...prWithReviews
					}
				}
			}
			reviewRequested: search(query: $reviewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...pr
					}
				}
			}
		}
```
   Variables are always these 6 keys, **even if undeclared by the chosen operation** (`number` is sent as
   `0` with the headRefName variant, and `headRefName` is sent as `""` with the number variant). **The
   server must ignore undeclared variables.**
   * `viewerQuery = "repo:OWNER/REPO state:open is:pr author:@me"`
   * `reviewerQuery = "repo:OWNER/REPO state:open review-requested:@me"`
   * `owner`, `repo`
   * `headRefName` = the branch without `owner:`
   * `number`

   Search results are `PullRequest` nodes; there is no `...on PullRequest` wrapper, only fragments on
   `PullRequest`, so the server must return PullRequest objects for `SearchResultItem` edges.
   The current PR is either `repository.pullRequest`, or the first `pullRequests` edge whose `HeadLabel()`
   equals the qualified head ref. A non-OPEN PR on the default branch is hidden.
6. `--json <fields>`: the fragments become
   `fragment pr on PullRequest{<PullRequestGraphQL(fields + isCrossRepository,headRepositoryOwner,headRefName)>}fragment prWithReviews on PullRequest{...pr}`,
   using the same prefix and searches (http.go:53-61). The output is
   `{"currentBranch": pr|null, "createdBy": [...], "needsReview": [...]}`.

Response fields read for rendering (status.go:200-312):
* `number, title, headLabel, state, isDraft`
* checks, from the counts or nodes
* `reviewDecision` (`CHANGES_REQUESTED|APPROVED|REVIEW_REQUIRED`), `latestReviews`,
  `baseRef.branchProtectionRule.requiredApprovingReviewCount`
* `mergeable`, `baseRef.branchProtectionRule.requiresStrictStatusChecks` + `mergeStateStatus` (`BEHIND`
  shows "Not up to date")
* `autoMergeRequest`
* search `issueCount`, under the alias `totalCount`


All paths are relative to the `cli` repo root (gh v2.102.0). REST paths below are relative to the REST base (`https://api.github.com/`, `https://HOST/api/v3/` for GHES, `http://api.github.localhost/` for `github.localhost`). GraphQL requests are `POST` to the GraphQL endpoint with JSON body `{"query": "...", "variables": {...}}` (go-gh `pkg/api/graphql_client.go:69`). No `operationName` key is sent; the operation name is only inside the query text.

## 3.4 Pull request write commands (merge/close/review)

### Conventions used in 3.4–3.6

**How struct queries become query text.** Most queries here are built from Go structs by `cli/shurcooL-graphql`. The text is generated like this, and the test fixtures confirm it (for example `mutation PullRequestUpdate($input:UpdatePullRequestInput!){updatePullRequest(input: $input){__typename}}`):
- Variable definitions are sorted alphabetically and written with no separator: `($input:MergePullRequestInput!)` or `($name:String!$owner:String!$tagName:String!)`. A Go pointer type (a nil `*githubv4.String`) becomes a nullable type such as `$endCursor:String`. Any other type gets `!`. The type name is the Go type's name. That is why gh sends `EnablePullRequestAutoMergeInput!`: it comes from the local wrapper type in `pkg/cmd/pr/merge/http.go:43`.
- Selections are separated by commas with no spaces: `{nodes{name,tagName},pageInfo{hasNextPage,endCursor}}`. Field names are the lowerCamelCase form of the Go field (`ClientMutationID` becomes `clientMutationId`, `DatabaseID` becomes `databaseId`) unless a `graphql:"..."` tag gives the text verbatim.
- The input object's JSON comes from `githubv4` input structs, whose optional (pointer) fields use `omitempty`. The githubv4 field JSON names below have been checked against `githubv4@48295856cce7` (`input.go`, `enum.go`).

**Headers on REST and GraphQL requests.** Requests made through `api.Client` (`api/client.go:413`) carry `X-GitHub-Api-Version: 2022-11-28`. The factory HTTP client also adds `User-Agent: GitHub CLI 2.102.0`, `Authorization: token <T>`, a default `Content-Type: application/json; charset=utf-8`, `Accept: application/vnd.github.merge-info-preview+json, application/vnd.github.nebula-preview` and `Time-Zone`. Each of these is added only if the request has not already set it (go-gh `pkg/api/http_client.go:166-197,215-232`). GraphQL requests from `api.Client.GraphQL/Query/Mutate` also carry `GraphQL-Features: merge_queue` (`api/client.go:23-24,111,124,136,148`).

**Status codes.** For REST through `api.Client`, any 2xx is success and anything else becomes an `HTTPError`. 204 and 205 mean no body is decoded (go-gh `pkg/api/rest_client.go:78-118`). For GraphQL, the HTTP status must be 2xx, and a non-empty `errors` array is turned into an error even when `data` is present (go-gh `pkg/api/graphql_client.go:85-107`).

**Path escaping.** Path segments built with `internal/safeurl` are each run through `url.PathEscape` (`internal/safeurl/safeurl.go:135-145`). A component that itself contains `/`, such as `heads/feature/x`, is therefore sent as a single segment with `%2F`.

---


### gh pr merge

Flags (`pkg/cmd/pr/merge/merge.go:174-185`):
- `--merge|-m`, `--squash|-s`, `--rebase|-r`: you can give at most one (`merge.go:104-121`).
- `--admin`, `--auto` and `--disable-auto` are mutually exclusive (`merge.go:129-136`).
- `--delete-branch|-d`, `--subject|-t`, `--body|-b`, `--body-file|-F`, `--match-head-commit SHA`, `--author-email|-A`.
- `--repo` with no argument is an error (`merge.go:96-98`). `--repo` also turns off local branch handling (`CanDeleteLocalBranch = !Changed("repo")`, `merge.go:124`).
- If no method flag is given, `MergeStrategyEmpty=true` and `MergeMethod` keeps its zero value, `PullRequestMergeMethodMerge` (`merge.go:117-118`, `http.go:15`).

**Requests, in order.**

1. **PR lookup (finder).** Fields requested: `id, number, state, title, lastCommit, mergeStateStatus, headRefName, baseRefName, headRefOid, isCrossRepository, isInMergeQueue, isMergeQueueEnabled` (`merge.go:571`). The finder always adds `id` and `number` (`pkg/cmd/pr/shared/finder.go:210`).
   - Because `isInMergeQueue` and `isMergeQueueEnabled` are requested, the finder first runs `PullRequestFeatures()` (`finder.go:212-226`). This happens on **every host, github.com and `github.localhost` included**: the `IsEnterprise` short-circuit is commented out (`internal/featuredetection/feature_detection.go:226-231`).
   - It sends two concurrent queries through a client with a 24h on-disk cache. The `X-GH-CACHE-TTL: 24h0m0s` request header is set and is not stripped before sending (`api/http_client.go:143-150`). Within the TTL the server may not see these queries at all.
     ```graphql
     query PullRequest_fields{PullRequest: __type(name: "PullRequest"){fields(includeDeprecated: true){name}},StatusCheckRollupContextConnection: __type(name: "StatusCheckRollupContextConnection"){fields(includeDeprecated: true){name}}}
     query PullRequest_fields2{WorkflowRun: __type(name: "WorkflowRun"){fields(includeDeprecated: true){name}}}
     ```
   - `MergeQueue` is true if and only if `PullRequest` has a field named `isInMergeQueue` (`feature_detection.go:271-275`). If it is false, `isInMergeQueue` and `isMergeQueueEnabled` are dropped from the PR query (`finder.go:221-224`) and both read as false.
   - `lastCommit` is used by `warnIfDiverged` as `Commits.Nodes[last].Commit.OID` (`merge.go:225-234`). The finder query itself is documented in another section.

2. **Local checks. These send no HTTP requests.**
   - `merged := pr.state == "MERGED"` (`merge.go:590`).
   - `autoMerge := --auto && !isImmediatelyMergeable(mergeStateStatus)`. The immediately mergeable states are `CLEAN`, `HAS_HOOKS` and `UNSTABLE` (`merge.go:593,826-833`). So **`--auto` on a CLEAN PR sends `mergePullRequest`, not `enablePullRequestAutoMerge`**.
   - `mergeQueueRequired := isMergeQueueEnabled` (`merge.go:595`).
   - If `isInMergeQueue` is true, gh prints "already queued to merge" and **exits 0** (`merge.go:214-220,167-169`).
   - `canMerge` (`merge.go:242-278`):
     - With a merge queue, `-d` is rejected ("Cannot use `-d` or `--delete-branch` when merge queue enabled").
     - Otherwise `mergeStateStatus` `BLOCKED` or `BEHIND` (unless `--admin`), or `DIRTY` (always), causes exit 1 (SilentError), unless `autoMerge` or `merged` is set.
   - **`--admin` does not change any request.** It only skips the client-side BLOCKED/BEHIND check (`merge.go:779-796`) and bypasses the merge queue path (`merge.go:550-552`).

3. **`--disable-auto` path** (`merge.go:611-613`, `http.go:106-119`). This is the only request; gh returns right after it.
   ```graphql
   mutation PullRequestAutoMergeDisable($prID:ID!){disablePullRequestAutoMerge(input: {pullRequestId: $prID}){clientMutationId}}
   ```
   Variables: `{"prID": "<PR node id>"}`.

4. **Interactive only** (no method flag, TTY, no merge queue; `merge.go:307-345`):
   - `query RepositoryInfo` from `api/queries_repo.go:360-387`, sent as a literal string. gh reads `mergeCommitAllowed`, `rebaseMergeAllowed` and `squashMergeAllowed` to build the menu.
     ```graphql
     fragment repo on Repository { id databaseId name owner { login } sshUrl hasIssuesEnabled description hasWikiEnabled viewerPermission defaultBranchRef { name } }
     query RepositoryInfo($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { ...repo parent { ...repo } mergeCommitAllowed rebaseMergeAllowed squashMergeAllowed } }
     ```
   - If the user picks "Edit commit subject" or "Edit commit message" and no subject or body was given, gh sends this (`http.go:121-158`):
     ```graphql
     query PullRequestMergeText($method:PullRequestMergeMethod!$prID:ID!){node(id: $prID){...on PullRequest{viewerMergeHeadlineText(mergeType: $method),viewerMergeBodyText(mergeType: $method)}}}
     ```
     If the error text contains `Field 'viewerMergeHeadlineText' doesn't exist` or `Field 'viewerMergeBodyText' doesn't exist`, gh ignores the error. This is the old-GHES fallback (`http.go:150-153`).

5. **Merge mutation** (`http.go:47-104`, called from `merge.go:348`).
   - The input is built as `githubv4.MergePullRequestInput`:
     - `pullRequestId`: always sent.
     - `mergeMethod`: **always sent**, one of `MERGE`, `REBASE` or `SQUASH`. It defaults to `MERGE`, even when the merge queue sets the strategy.
     - `authorEmail`: only if `-A` is given.
     - `commitHeadline`: only if `--subject` is non-empty.
     - `commitBody`: sent if `--body` or `--body-file` is given, **even when empty**, or after editing in the editor.
     - `expectedHeadOid`: only if `--match-head-commit` is given.
   - Direct merge, used when the PR is not auto-merge and not merge-queue, or when `--admin` is given:
     ```graphql
     mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}
     ```
     Variables: `{"input":{"pullRequestId":"PR_x","mergeMethod":"SQUASH","commitHeadline":"...","commitBody":"...","expectedHeadOid":"<sha>","authorEmail":"..."}}`.
   - Auto-merge. This is used when `autoMerge` is true, **or** when the merge queue is required and `--admin` is not given. In the merge queue case gh forces `auto=true` and only warns if a method flag was passed (`merge.go:298-305`).
     ```graphql
     mutation PullRequestAutoMerge($input:EnablePullRequestAutoMergeInput!){enablePullRequestAutoMerge(input: $input){clientMutationId}}
     ```
     The variable `input` has the same flattened JSON keys as above, because the Go struct embeds `MergePullRequestInput`.
   - Only success or failure of the response is used. `clientMutationId` is never read.

6. **Local branch deletion. These are git commands, not HTTP** (`merge.go:380-514`).
   - This step is skipped if `autoMerge` is true, if `-d` was not requested (or the prompt was declined), if `--repo` was used, or if the local branch does not exist.
   - If the PR was already merged and gh can prompt and `-d` was not given, gh asks "Delete the branch locally?".
   - The order of operations is listed in the git section below. If the head branch is checked out in the current main worktree, gh runs `git checkout <base>` (or `git checkout -b <base> --track <remote>/<base>`), then `git pull --ff-only <remote> <base>` (a failure here is only a warning), then `git branch -D <head>`. `<remote>` is the git remote that matches the base repo.

7. **Remote branch deletion** (`merge.go:517-546`, `api/queries_pr.go:872-878`).
   - Skipped if `-d` was not requested, if the PR is cross-repository, or if `autoMerge` is true.
   - **If the PR was already MERGED, no request is sent, but "Deleted remote branch" is still printed** (`merge.go:523-545`).
   - Otherwise:
     ```
     DELETE repos/{owner}/{repo}/git/refs/heads%2F{branch}
     ```
     The path is `safeurl.JoinPath("repos", owner, repo, "git", "refs", "heads/"+branch)`, so the `/` after `heads` **and every `/` inside the branch name** are sent as `%2F`. For example, `feature/x` is sent as `refs/heads%2Ffeature%2Fx`. Go keeps the escaped form in `RawPath`, so this is what goes over the wire.
     - Success is any 2xx; GitHub returns 204.
     - **422 and 404 are both treated as "already deleted" and are not errors** (`merge.go:532-541`). Any other status is reported as `failed to delete remote branch`.
   - If deletion was requested interactively with no `-d`, the prompt is "Delete the branch locally and on GitHub?" or "Delete the branch on GitHub?" (`merge.go:666-684`).

**Git subprocesses** (`git/client.go`). If `GitClient.RepoDir` is set, each command is prefixed with `-C <dir>`.
- `git rev-parse --verify refs/heads/<headRefName>`: checks whether the local branch exists (`git/client.go:708-711`). Runs only without `--repo`.
- `git -c log.ShowSignature=false show -s --pretty=format:%H,%s HEAD`: the divergence warning (`git/client.go:412,429-431`). Runs only when no selector argument was given.
- `git remote -v` (and related): finds the remote for the base repo, for the merge-conflict hint and for deletion.
- `git worktree list --porcelain` (`git/client.go:269-279`), then `git rev-parse --show-toplevel` (`git/client.go:730-735`).
- `git worktree prune`: only for prunable worktrees (`git/client.go:294-302`).
- `git worktree remove -- <path>`: when the head branch is checked out in another linked worktree (`git/client.go:283-291`).
- `git checkout <base>` (`git/client.go:681-683`) **or** `git checkout -b <base> --track <remote>/<base>` (`git/client.go:694-697`).
- `git -c credential.helper= -c 'credential.helper=!"gh" auth git-credential' pull --ff-only <remote> <base>` (`git/client.go:143-162,896-901`).
- `git branch -D <headRefName>` (`git/client.go:668-670`).

### gh pr close

`pkg/cmd/pr/close/close.go`. It takes exactly one argument. Flags are `--comment|-c` and `--delete-branch|-d`. `DeleteLocalBranch = !Changed("repo")` (`close.go:50`).

1. **Finder.** Fields: `state, number, title, isCrossRepository, headRefName`, plus the implicit `id, number` (`close.go:68-71`). No feature detection runs.
   - If `state == "MERGED"`, gh exits 1 ("can't be closed because it was already merged").
   - If the PR is otherwise not open, gh prints a warning and exits 0. No mutation is sent (`close.go:77-83`).
2. **`--comment`** (`close.go:90-104`). This is sent **before** the close.
   ```graphql
   mutation CommentCreate($input:AddCommentInput!){addComment(input: $input){commentEdge{node{url}}}}
   ```
   Variables: `{"input":{"body":"<text>","subjectId":"<PR node id>"}}` (`api/queries_comments.go:56-80`).
3. **Close** (`api/queries_pr.go:767-784`):
   ```graphql
   mutation PullRequestClose($input:ClosePullRequestInput!){closePullRequest(input: $input){pullRequest{id}}}
   ```
   Variables: `{"input":{"pullRequestId":"<PR node id>"}}`.
4. **`--delete-branch`** (`close.go:113-161`).
   - Runs `git rev-parse --verify refs/heads/<head>`. This runs even with `--repo`.
   - If `DeleteLocalBranch` is set and the branch exists:
     - gh finds the current branch with `git symbolic-ref --quiet HEAD` (`git/client.go:223-224`).
     - If the current branch is the head branch, gh runs `api.RepoDefaultBranch`. The finder's repo is not an `*api.Repository`, so this always sends the `RepositoryInfo` GraphQL query shown under merge and reads `defaultBranchRef.name`. It then runs `git checkout <default>`.
     - Then it runs `git branch -D <head>`.
   - If the local branch does not exist, gh prints "Skipped deleting the local branch since current directory is not a git repository".
   - If the PR is cross-repository, the remote delete is skipped with a warning.
   - Otherwise gh sends `DELETE repos/{owner}/{repo}/git/refs/heads%2F{branch}`, escaped the same way as in merge. **Unlike merge, close does not tolerate 422 or 404**: any non-2xx is an error, `failed to delete remote branch` (`close.go:156-158`).

### gh pr review

`pkg/cmd/pr/review/review.go`. Flags are `--approve|-a`, `--request-changes|-r`, `--comment|-c`, `--body|-b` and `--body-file|-F`.
- `-r` and `-c` require a non-empty body (`review.go:106-119`).
- A body without a review type is an error.
- More than one review type is an error (`review.go:121-130`).

1. **Finder.** Fields: `id, number` (`review.go:149-152`).
2. **Mutation** (`api/queries_pr_review.go:249-274`). The struct tag has no space after the colon, so the text is `addPullRequestReview(input:$input)`:
   ```graphql
   mutation PullRequestReviewAdd($input:AddPullRequestReviewInput!){addPullRequestReview(input:$input){clientMutationId}}
   ```
   Variables: `{"input":{"pullRequestId":"<id>","event":"APPROVE"|"REQUEST_CHANGES"|"COMMENT","body":"<text>"}}`.
   - `event` and `body` are **always present**. For `--approve` with no body, gh sends `"body":""`.
   - `commitOID`, `comments` and `threads` are never sent.
   - gh only checks for success.

---

## 3.5 Release commands

The release REST paths use `safeurl`, so a tag containing `/` is sent as `%2F` inside one segment, for example `releases/tags/v1%2Fbeta`.

The REST release object that gh decodes is `pkg/cmd/release/shared/fetch.go:44-85`. Keys read:
- Release: `id` (int64), `node_id`, `tag_name`, `name`, `body`, `draft`, `prerelease`, `immutable`, `created_at`, `published_at` (nullable), `target_commitish`, `url` (the API URL), `upload_url`, `tarball_url`, `zipball_url`, `html_url`, `assets[]`, `author{node_id,login}`.
- Each asset: `node_id`, `name`, `label`, `size`, `digest` (nullable string), `state`, `url`, `created_at`, `updated_at`, `download_count`, `content_type`, `browser_download_url`.

### gh release create

Source: `pkg/cmd/release/create/create.go` and `create/http.go`.

Flags:
- `--draft|-d`, `--prerelease|-p`, `--target`, `--title|-t`, `--notes|-n`, `--notes-file|-F`, `--discussion-category`, `--generate-notes`, `--notes-start-tag`.
- `--latest`: a nil-able bool, so it is either unset or explicitly true or false.
- `--verify-tag`, `--notes-from-tag`, `--fail-on-no-commits`.
- Upload concurrency is fixed at 5 (`create.go:190`).

**No feature detection and no `IsEnterprise` branching.** The only host-dependent part is the REST base and the `upload_url` that the server returns.

Requests, in order:

1. **`--fail-on-no-commits`** (`create.go:239-247`, `http.go:238-267`):
   - `GET repos/{o}/{r}/releases/latest`. A 404 means "new", and gh skips the next request.
   - `GET repos/{o}/{r}/compare/{latestTag}...HEAD?per_page=1`. The `tag...HEAD` text is one path-escaped segment. gh reads `status`, and continues only if it is `"ahead"`.

2. **No tag argument (interactive only)**: `GET repos/{o}/{r}/tags?per_page=5`. gh reads `[].name` (`create.go:250-281`, `http.go:60-73`). If the user picks an existing tag, `existingTag=true`.

3. **`--verify-tag`**, when the tag was not picked from the list (`create.go:283-292`, `http.go:41-58`):
   ```graphql
   query RepositoryFindRef($name:String!$owner:String!$tagName:String!){repository(owner: $owner, name: $name){ref(qualifiedName: $tagName){id}}}
   ```
   Variables: `{"name":"<repo>","owner":"<owner>","tagName":"refs/tags/<tag>"}`. The tag exists if `ref.id` is non-empty, so `ref: null` means it does not. If it is missing, gh aborts.

4. **Local tag check** (only without `--repo`; `create.go:295-321`).
   - Runs `git tag --list <tag> --format=%(contents)` and `git tag --list <tag> --format=%(contents:signature)` (`create.go:570-603`).
   - If a local tag exists, and there is no `--target`, no `--verify-tag`, and no tag picked from the list, gh sends the same `RepositoryFindRef` query. If the tag is missing on the remote, gh errors: "exists locally but has not been pushed".
   - **gh never uses REST to check whether a tag exists.** If the tag does not exist on the remote, the server is expected to create it from `target_commitish`, or from the default branch, when the release is created or published.

5. **Interactive notes** (only if no notes flag is given and gh can prompt; `create.go:323-446`). gh sends:
   ```
   POST repos/{o}/{r}/releases/generate-notes
   {"tag_name":"<tag>"[,"target_commitish":"<target>"][,"previous_tag_name":"<notes-start-tag>"]}
   ```
   It reads `name` and `body`. **A 404 is treated as "not implemented" and is ignored** (`http.go:75-108`). The local fallbacks are:
   - `git describe --tags --abbrev=0 <headRef>^`
   - `git -c log.ShowSignature=false log --first-parent --reverse --pretty=format:%B%x00 <prev>..<headRef>`

6. **Building the create body** (`create.go:448-495`). The JSON is built with `json.Marshal` of a map, so the keys come out in alphabetical order:
   - Always: `tag_name`, `draft` (bool), `prerelease` (bool).
   - `name`: only if non-empty.
   - `body`: only if non-empty.
   - `target_commitish`: only if `--target` is given.
   - `make_latest`: **a string `"true"` or `"false"`**, sent only if `--latest` was given.
   - `discussion_category_name`: only if given.
   - With `--generate-notes`:
     - Without `--notes-start-tag`, gh adds `"generate_release_notes": true` and the server generates the notes.
     - With `--notes-start-tag`, gh first sends `POST .../releases/generate-notes` with `previous_tag_name` (404 is ignored). It then sets `body`, as `<notes>\n<generated>` if notes were also given, and sets `name` from the generated `name` if no title was given.
   - `--notes-from-tag`: `body` is the local tag annotation, with any user notes before it.

7. **Assets and not `--draft`** (`create.go:497-512`):
   - First gh sends `HEAD repos/{o}/{r}/releases/tags/{tag}` (`http.go:110-138`):
     - **200 means stop**, with "a release with the same tag name already exists".
     - **404 means continue.**
     - Any other 2xx is an "unexpected HTTP" error.
     - Any other status is an error.
   - Then gh forces `"draft": true` in the create body. This is "draftWhileUploading".
   - Note that `discussion_category_name` and `make_latest` stay in the draft create body in this case.

8. **Create**: `POST repos/{o}/{r}/releases` with the JSON body above (`http.go:140-180`).
   - Expected response: 201 with a release object. gh reads `url`, `upload_url` and `html_url`.
   - **If the response is 404 and the `X-Oauth-Scopes` header is present but does not contain `workflow`**, gh prints a hint to add the `workflow` scope and exits 1 (`http.go:167-176,217-235`). If `X-Oauth-Scopes` is missing or empty, gh assumes the token is fine and shows the normal error.

9. **Asset upload** (`create.go:543-554`, `shared/upload.go:111-208`). Uploads run concurrently, at most 5 at a time.
   - The upload URL is `upload_url` cut at the first `{`, but only if the index of `{` is greater than 0. An RFC 6570 template like `https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}` must therefore be returned. gh does not construct the upload host itself.
   - Request: `POST <upload_url_base>?label=<label>&name=<basename>`. Query keys are sorted by `url.Values.Encode`, and **`label` is always present, even when empty (`label=`)**.
   - Headers:
     - `Content-Type`: the MIME type from the file extension. Overrides: `.zip` is `application/zip`, `.js` is `application/javascript`, `.tar` is `application/x-tar`, `.tgz` and `.tar.gz` are `application/x-gtar`, `.bz2` is `application/x-bzip2`, `.dmg` is `application/x-apple-diskimage`, `.rpm` is `application/x-rpm` and `.deb` is `application/x-debian-package`. Otherwise gh uses `mime.TypeByExtension`, falling back to `application/octet-stream` (`upload.go:75-101`).
     - `Content-Length` is the file size.
   - The body is the raw file bytes.
   - An asset argument of `path#Label` sets the label (`upload.go:36-73`).
   - Authorization: a token is attached if the upload host normalizes to a logged-in host. For example, `uploads.github.localhost` normalizes to `github.localhost` (`api/http_client.go:154-189`).
   - Success is 2xx, and gh decodes a release asset from the response.
   - **Retries:** up to 3 more attempts, 200ms apart, on a 5xx or a transport error (`upload.go:130-155`).

10. **Publish**, only when `draftWhileUploading` (`create.go:556-562`, `http.go:182-206`):
    ```
    PATCH <release "url" from step 8>
    {"draft":false[,"discussion_category_name":"..."][,"make_latest":"true"|"false"]}
    ```
    `prerelease`, `name` and `body` are **not** resent. gh reads `html_url` from the response.

11. **Cleanup** (`create.go:533-541`, `http.go:208-213`). If an upload or the publish fails while `draftWhileUploading`, gh sends `DELETE <release "url">` (204 expected).

12. Output: gh prints `html_url` to stdout.

`--draft` with assets creates the draft, uploads, and sends no PATCH. Without assets there is no HEAD, no upload and no PATCH.

### gh release list

`pkg/cmd/release/list/{list.go,http.go}`. This command uses **GraphQL only**.

Flags:
- `--limit|-L` (default 30)
- `--exclude-drafts` and `--exclude-pre-releases`. These filter **on the client side**.
- `--order|-O asc|desc` (default desc)
- `--json` with fields `name, tagName, isDraft, isLatest, isPrerelease, isImmutable, createdAt, publishedAt` (`http.go:15-24`)

1. **Feature detection**, always, on every host including github.com and `github.localhost`, with a 24h-cached client (`list.go:81-89`, `feature_detection.go:492-520`):
   ```graphql
   query Release_fields{Release: __type(name: "Release"){fields{name}}}
   ```
   `ImmutableReleases` is true if and only if some field is named `immutable`. Unlike the PR detection, this uses `fields` without `includeDeprecated`.

2. **List query.** This is paginated; gh stops when it has collected `limit` items or when `hasNextPage` is false (`http.go:41-116`). `perPage = min(limit, 100)`, and `direction` is `ASC` or `DESC`.
   - If `immutable` exists:
     ```graphql
     query RepositoryReleaseList($direction:OrderDirection!$endCursor:String$name:String!$owner:String!$perPage:Int!){repository(owner: $owner, name: $name){releases(first: $perPage, orderBy: {field: CREATED_AT, direction: $direction}, after: $endCursor){nodes{name,tagName,isDraft,immutable,isLatest,isPrerelease,createdAt,publishedAt},pageInfo{hasNextPage,endCursor}}}}
     ```
   - If it does not exist, the query is identical except that `immutable` is left out of `nodes{...}` (`http.go:122-198`):
     ```graphql
     ...nodes{name,tagName,isDraft,isLatest,isPrerelease,createdAt,publishedAt}...
     ```
   - Variables: `{"direction":"DESC","endCursor":null,"name":"<repo>","owner":"<owner>","perPage":30}`. On later pages `endCursor` is set to the previous page's `pageInfo.endCursor`.
   - `publishedAt` decodes to a non-pointer `time.Time`. If it is null, `--json` outputs `"0001-01-01T00:00:00Z"`, and the table falls back to `createdAt`.
   - If there are no results and `--json` was not given, gh prints "no releases found" and exits 0.

### gh release view

`pkg/cmd/release/view/view.go`. `--json` fields are `apiUrl, author, assets, body, createdAt, databaseId, id, isDraft, isPrerelease, isImmutable, name, publishedAt, tagName, tarballUrl, targetCommitish, uploadUrl, url, zipballUrl` (`shared/fetch.go:23-42`).

The mapping from JSON field to REST key:
- `id` is `node_id`; `databaseId` is `id`.
- `url` is `html_url`; `apiUrl` is `url`.
- `isImmutable` is `immutable`.
- `author` is `{id: node_id, login}`.
- Each asset is `{url: browser_download_url, apiUrl: url, id: node_id, name, label, size, digest, state, createdAt, updatedAt, downloadCount, contentType}`.

There is no feature detection.

- **No tag:** `GET repos/{o}/{r}/releases/latest` (`fetch.go:228-234`). A 404 gives the error `release not found`.
- **With a tag** (`fetch.go:183-225`). gh runs two lookups **concurrently**, and the first success wins:
  - (a) `GET repos/{o}/{r}/releases/tags/{tag}`.
  - (b) A GraphQL lookup for draft releases (`fetch.go:237-270`):
    ```graphql
    query RepositoryReleaseByTag($name:String!$owner:String!$tagName:String!){repository(owner: $owner, name: $name){release(tagName: $tagName){databaseId,isDraft}}}
    ```
    If `release` is non-null and `isDraft` is true, gh then sends `GET repos/{o}/{r}/releases/{databaseId}`. Otherwise this lookup counts as "not found".
  - If both lookups fail, a not-found from (b) takes precedence.
  - In practice the server should support `Repository.release(tagName:)` finding a **draft by its pending tag name**. The REST `releases/tags/{tag}` endpoint only needs to return published releases.
- `--web` opens `html_url`.
- **TTY output dereferences `published_at` for non-drafts**, so a non-draft release with `published_at: null` will crash gh (`view.go:144`).

---

## 3.6 gh api

Source: `pkg/cmd/api/{api.go,http.go,fields.go,pagination.go}`. It uses `api.NewHTTPClient` directly. The `Authorization`, `User-Agent` and default-header rules above still apply, but gh sets `Accept: */*` before the transport runs, so the preview Accept header is not used (`http.go:88-90`). The default `Content-Type: application/json; charset=utf-8` from the transport is added to **every** request that does not set one, GETs included.

### URL building (`http.go:16-36`, `api.go:311-321,575-615`)

- **Host:** `--hostname` if given, otherwise `cfg.Authentication().DefaultHost()`, which is `GH_HOST` or the configured default (`api.go:417-421`). The host is **not** taken from the current repo.
  - `--hostname` must not contain `/` or `:` (`internal/ghinstance/host.go:36-44`).
  - If the config has `hosts.<host>.api_host`, the URL's host is replaced with it (`api.go:423`, `http.go:26-35,164-175`).
- **Absolute URLs:** if the endpoint contains `://`, it is used verbatim.
- **GraphQL:** only when the endpoint is exactly the string `graphql`, checked before placeholders are filled (`api.go:317`). The URL is `ghinstance.GraphQLEndpoint(host)`: `https://api.github.com/graphql`, `https://HOST/api/graphql` for GHES, or `http://api.github.localhost/graphql`.
- **Otherwise:** `ghinstance.RESTPrefix(host) + strings.TrimPrefix(path, "/")`. Only **one** leading slash is removed. The path is **not escaped**. `RESTPrefix` uses `http://api.<host>/` only when the host is exactly `github.localhost` (case-insensitive). A `--hostname x.github.localhost` resolves to `https://api.x.github.localhost/`, because it is not enterprise and not exactly localhost (`host.go:59-70`).
- **Placeholders:** the regex is `(\:(owner|repo|branch)\b|\{[a-z]+\})` (`api.go:575`). It replaces `{owner}` or `:owner` with the base repo owner, `{repo}` or `:repo` with the repo name, and `{branch}` or `:branch` with the current git branch.
  - `{branch}` is an error if `GH_REPO` is set.
  - Other `{word}` matches are left as they are.
  - Placeholders are also filled in `-F` values (`fields.go:181`), but not in `-f` values.

### Fields and method (`fields.go:16-183`, `api.go:322-331`)

- **`-f key=value`** (raw) always gives a string. All `-f` fields are parsed before any `-F` fields.
- **`-F key=value`** (magic):
  - `@file` or `@-` reads the file or stdin **as a string**.
  - A value that `strconv.Atoi` accepts becomes an int.
  - `true` and `false` become bools, and `null` becomes JSON null.
  - Otherwise gh fills placeholders and uses a string.
- **Nesting:** `a[b][c]=v` builds nested objects. `a[]=v` appends to an array, and `a[]` with no `=` gives an empty array. `a[][k]=v` builds an array of objects, starting a new object when key `k` repeats. Setting the same scalar key twice is an error.
- **Default method:** `-X` defaults to `GET`. If `-X` is **not** given and there is at least one field **or** `--input`, the method becomes **POST**.
  - This applies to `--paginate` too: the paginate guard compares the *flag value* `GET` (`api.go:248`), so `gh api --paginate path -f k=v` sends a POST.
- **GET with fields:** the fields go into the query string through `addQuery` (`http.go:114-160`):
  - Strings are added as-is, ints with `%d`, bools as `true` or `false`, and nil as an empty string.
  - Arrays become `key[]=v` for each element.
  - **Nested maps are flattened using only the sub-key name; the parent key is dropped.**
  - `url.Values.Encode` sorts the keys. The separator is `?`, or `&` if the path already has a `?`.
- **Non-GET:** the body is the JSON of the field map, with `Content-Type: application/json; charset=utf-8` unless `-H` sets one.
  - For `graphql`, the keys `query` and `operationName` stay at the top level, and every other key moves under `variables`. `variables` is left out if it would be empty (`http.go:95-112`).
  - `-F` ints and bools therefore arrive as typed GraphQL variables.
- **`--input file|-`:** the body is the raw file content. Fields go into the URL query string instead of the body (`api.go:373-384`).
  - For a regular file, gh adds `Content-Length: <size>` as the first header. It is applied as `req.ContentLength`, not as a header (`http.go:75-80`).
  - For stdin, gh sends no length, so the body is chunked.
- **`-H 'Name: value'`:** the name is split at the first `:`, and the value is trimmed. Headers are added with `Header.Add`, so a repeated name gives multiple values. A missing `:` is an error.
- **`-p/--preview a,b`:** appends `Accept: application/vnd.github.a-preview+json, application/vnd.github.b-preview` (`api.go:386-388,728-734`).
- The method is upper-cased (`http.go:64`).
- **`--cache 1h`** turns on the client's disk cache and sets the `X-GH-CACHE-TTL` request header, which is also sent over the wire.

### Pagination (`api.go:358-470`, `pagination.go`)

`--paginate` cannot be combined with `--input`. With a non-GET `-X` it is only allowed for `graphql`.

**REST:**
- gh appends `per_page=100`, unless `per_page` is already in the fields or in the path's query string (`pagination.go:94-110`).
- After each response, gh parses the `Link` header with this regex (`pagination.go:15`, the same as `api/client.go:28`), and takes the first match whose rel is `next`:
  ```
  <([^>]+)>;\s*rel="([^"]+)"
  ```
- The `<...>` URL is then requested **verbatim**, as an absolute URL with the original method and headers. The body or fields are dropped (`api.go:439-442`).
- Pagination stops when there is no `rel="next"`.
- Without `--slurp` or `--include`, JSON array pages are spliced into one array: the `]` and `[` between pages are replaced by `,`, and an empty page `[]` by a space (`pagination.go:114-150`).

**GraphQL:**
- The request is a POST, since `query` is a field.
- After each 200 JSON response, gh scans the body as a token stream for the **first** object whose key is `pageInfo` and that directly contains both `hasNextPage` (bool) and `endCursor` (string), in either order (`pagination.go:26-92`).
- If `hasNextPage` is true, gh sets the variable `endCursor` to that string and sends the next request.
- gh stops when `hasNextPage` is false, when there is no `pageInfo`, or when `endCursor` is null, because a null is not a string token and so is never recorded.
- Only the first `pageInfo` found in document order is followed. The query has to declare `$endCursor: String` itself; gh does not rewrite it.

**`--slurp`** requires `--paginate` and cannot be combined with `--jq` or `--template`. It wraps every page, whether array or object, in an outer JSON array: `[` page1 `,` page2 ... `]`, or `[]` if there are none (`pagination.go:152-241`).

### Output, errors, and exit codes (`api.go:477-573`)

- **`-i/--include`:** prints `<proto> <status>`, then the headers sorted alphabetically as `Name: v1, v2\r\n` (skipping `Status`), then `\r\n`.
- A **204** response prints no body.
- **JSON detection:** the `Content-Type` must match `[/+]json(;|$)`.
- **Error parsing** applies when the response is JSON and either the endpoint is `graphql` (**any status, including 200**) or the status is ≥ 400. It is skipped for `-X HEAD` (`parseErrorResponse`, `api.go:655-726`). The rules, in order:
  1. If `errors` is a JSON string, the message is `"<errors> (<message>)"`, or just `"<errors>"` if there is no message.
  2. Otherwise, if `message` is non-empty, the message is `"<message> (HTTP <status>)"`.
  3. Otherwise, if `errors` is an array, the message is each element's `.message` (for objects) or the string itself, joined with `\n`. This is the GraphQL `{"errors":[{"message":...}]}` case.
- **The body is still printed in full to stdout.** `--jq` and `--template` are skipped when there is a server error.
- gh then writes `gh: <message>` to stderr. If there was no parsed message but the status is greater than 299, it writes `gh: HTTP <status>`. It may add a scopes hint based on `X-Accepted-Oauth-Scopes` and `X-Oauth-Scopes`, and `Authorize in your web browser: <url>` when an `X-GitHub-SSO` URL was captured.
- **The exit code is 1** (SilentError), and pagination stops.
- **GraphQL errors exit 1 even when HTTP is 200 and `data` is present.**
- For 401, `gh api` does **not** print "Try authenticating with" because it handles the response itself.
- Other exit codes from `internal/ghcmd/cmd.go:44-48`: 0 for OK, 1 for an error, 2 for cancel, 4 for auth (not logged in), 8 for pending.
- `--silent` throws the body away. `--verbose` sends the HTTP dump to stdout instead of the body.
- A non-JSON body is copied through as-is. On a TTY, binary content or terminal escape sequences cause an error unless `--allow-escape-sequences` is given.

## 4. Response headers, version detection, error shapes, GraphQL wire format

### 4.1 Response headers gh reads

| Header | Required? | Where | Effect |
|---|---|---|---|
| `Content-Type` matching regex `[/+]json($\|;)` | **Yes** for JSON bodies | `go-gh/pkg/api/http_client.go:37,257-271`, `go-gh/pkg/api/errors.go:110` | JSON responses are run through an ASCII control-char sanitizer; **error bodies are only parsed if Content-Type is JSON** (otherwise the message becomes the HTTP status line). Use `application/json; charset=utf-8`. |
| `X-OAuth-Scopes` | Optional | `pkg/cmd/auth/shared/oauth_scopes.go`, `api/client.go:326-330,361-410` | Login scope check (absent ⇒ check skipped); `auth status` display; on 4xx (except 422) combined with `X-Accepted-OAuth-Scopes` to print "This API operation needs the X scope" hint. |
| `X-Accepted-OAuth-Scopes` | Optional | `api/client.go:309,327,356` | Only for the 4xx scope hint. |
| `X-GitHub-SSO` | Optional | `pkg/cmd/factory/default.go:24,207,293-305` | If present, `url=([^;]+)` is extracted and shown as a SAML SSO challenge. Never send it. |
| `Link` | Needed for REST pagination | `api/client.go:28,193-198` (`<([^>]+)>;\s*rel="([^"]+)"`), `pkg/cmd/api` | `rel="next"` URL is followed verbatim (absolute URL). |
| `X-GitHub-Request-Id` | **Not read** by any non-test code in cli or go-gh | — | Optional. |
| `x-ratelimit-remaining` | Not read for our commands (only `gh skills search`) | — | Optional. |
| Version headers (`X-GitHub-Enterprise-Version`, `X-GitHub-Media-Type`) | **Not read** | — | Optional. |

Status codes: REST helpers treat any 2xx as success; 204 and 205 are "no body" (`go-gh/pkg/api/rest_client.go:113-119`). **GraphQL struct-tag queries (shurcooL, used for most `client.Query`/`client.Mutate` calls) require exactly `200 OK`** (`shurcooL/graphql.go`: `if resp.StatusCode != http.StatusOK { return fmt.Errorf("non-200 OK status code: ...") }`); string queries via `client.GraphQL` accept any 2xx (`go-gh/pkg/api/graphql_client.go:85-88`). Always answer GraphQL with 200.

### 4.2 GHES version detection

`internal/featuredetection/feature_detection.go:resolveEnterpriseVersion` → `GET /api/v3/meta`, reads JSON `installed_version` (string, parsed by `hashicorp/go-version`). Only called for **GHES hosts**, from:

* `SearchFeatures()` — error is **fatal** to the calling command (empty/invalid `installed_version` ⇒ `version.NewVersion("")` error). `>= 3.18.0` ⇒ advanced issue search enabled, then introspection of `SearchType` enum.
* `ProjectsV1()` — errors ignored; `< 3.17.0` ⇒ Projects (classic) supported. Any value `>= 3.17.0` ⇒ unsupported (simplest: gh then never queries `projectCards`).
* `ActionsFeatures()` — `>= 3.21.0` ⇒ workflow dispatch `return_run_details`.

**Recommendation:** `GET /api/v3/meta` → `200 {"installed_version":"3.20.0", "verifiable_password_authentication": true}`. 3.20.0 is ≥3.18 (search works) and ≥3.17 (no classic projects). See §3 "Feature detection" for the introspection queries and the answers that give the simplest code path.

### 4.3 REST error body shape

`go-gh/pkg/api/errors.go:100-171`:

```json
{ "message": "Validation Failed",
  "errors": [ {"resource":"Label","field":"name","code":"already_exists"} | {"message":"..."} | "plain string" ],
  "documentation_url": "..." }
```

* `message` (string) and `errors` (array of either strings or objects `{resource, field, code, message}`) are parsed. For object errors with `code` other than `"custom"` the displayed text is `"<resource>.<field> <code-text>"` with `missing|missing_field → "is missing"`, `invalid|unprocessable → "is invalid"`, `already_exists → "already exists"`. With `code:"custom"` the `message` is used.
* Rendered as `HTTP <status>: <message> (<url>)`.
* Code that branches on REST errors: `label create --force` requires exactly `422` with one error `{"field":"name","code":"already_exists"}` (`pkg/cmd/label/create.go:192`); release create/view treat 404 as "not found"; `pr merge --delete-branch` treats 422/404 on branch deletion as "already deleted" (`pkg/cmd/pr/merge/merge.go:536`); `repo view` README 404 ⇒ no README (`pkg/cmd/repo/view/http.go:42`).

### 4.4 GraphQL response/error shape

Request: `POST` with JSON `{"query": "...", "variables": {...}}`. `variables` may be `null` (string-query path, e.g. `{"query":"query UserCurrent{viewer{login}}","variables":null}`) or **omitted** (shurcooL path with no variables). An `operationName` key is never sent; the operation name is embedded in the query text.

Response: `{"data": {...}, "errors": [{"message": "...", "type": "NOT_FOUND", "path": ["repository"], "locations": [{"line":1,"column":2}], "extensions": {...}}]}` (`go-gh/pkg/api/errors.go:40-98`).

* gh prints `GraphQL: <message> (<path joined with .>), ...`.
* **The GitHub-specific top-level `type` field is load-bearing**: `api/queries_repo.go:518-528` (RepositoryNetwork, used for base-repo/fork resolution) tolerates errors only if **every** error has `type == "NOT_FOUND"`; elsewhere `GraphQLError.Match(type, path)` is used. A missing repo must therefore return `data.repository: null` plus `{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve to a Repository with the name 'o/r'."}`.
* `data` is decoded even when `errors` is present (partial data); errors are then returned.
* **Strict decoding (shurcooL):** `shurcooL/internal/jsonutil/graphql.go:105` fails with `struct field for "x" doesn't exist in any of N places to unmarshal` if the response contains a key that was not selected. The server must return exactly the selected fields (standard GraphQL behaviour) — including correct alias names and `__typename` only when selected.

### 4.5 GraphQL query text conventions the server's parser/validator must accept

* shurcooL-generated queries are minified: `query Name($a:String!$b:Int!){repository(owner:$a,name:$b){...}}` — variable definitions are **not comma-separated**; inline fragments are written `...on User{login}`; aliases like `Issue: __type(name: "Issue")` appear.
* Variable **type names** come from Go types (`shurcooL/query.go:41-80`, `githubv4` scalars): `String!`, `Int!`, `Boolean!`, `ID!`, `URI!`, `DateTime!`, `GitObjectID!`, `[ID!]`, `[String!]`, plus input/enum names such as `CreateIssueInput!`, `UpdateIssueInput!`, `AddCommentInput!`, `MergePullRequestInput!`, `[IssueState!]`, `[PullRequestState!]`, `IssueOrder`, `RepositoryVisibility`... If the server validates variable types, **every such type name must exist in its schema with that exact name.** Rules (`shurcooL/query.go` `writeArgumentType`): the declared type is the Go type's *name*; non-pointer ⇒ `!` appended, pointer ⇒ nullable, slice ⇒ `[T!]`; and **a plain Go `string` is declared as `ID!`** (hack for githubv4#12). So the server should be lenient about `ID` vs `String` variable/argument compatibility (e.g. treat `ID` and `String` as mutually assignable in the "variables in allowed position" validation rule), or skip that rule.
* Hand-written queries use multi-line text with tabs and fragments defined before the operation (e.g. `fragment pr on PullRequest{...}\n\t\tquery PullRequestList(\n\t\t\t$owner: String!, ...`), variable defaults (`$states: [IssueState!] = OPEN`), and comma-separated selection sets (`{number,title,state}`).
* Introspection: gh issues `__type(name: "X") { fields(includeDeprecated: true) { name } }`, `__type(name:"ProjectV2"){fields{name args{name}}}`, `__type(name:"SearchType"){enumValues(includeDeprecated: true){name}}` — the server must support `__type` with at least `fields{name,args{name}}` and `enumValues{name}`. The PR feature-detection comment notes GitHub only allows two `__type` per query, so gh splits them.

### 4.6 GraphQL validation leniency the server MUST have (verified, see Appendix A)

GitHub's GraphQL server accepts several things that a spec-strict validator (graphql-js, graphql-core, async-graphql/juniper defaults) rejects. A forge server must relax these:

1. **`OverlappingFieldsCanBeMerged` / SameResponseShape across disjoint inline fragments.** gh sends
   * `issueOrPullRequest(number:$number){__typename ...on Issue{...state...} ...on PullRequest{...state...}}` — `Issue.state: IssueState!` vs `PullRequest.state: PullRequestState!` (`pkg/cmd/issue/shared/lookup.go:160`; `gh issue view/close/reopen/comment/edit`).
   * `requestedReviewer{__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}` — `User.name: String` vs `Team.name: String!` (`api/query_builder.go` reviewRequests; every `gh pr view`, `pr edit`, `pr list --json reviewRequests`).
   Recommended: disable that rule or only enforce it when the parent types can overlap. (Alternative hack: make the conflicting fields' types identical in the schema, e.g. `Team.name: String`, but the `IssueState`/`PullRequestState` conflict cannot be avoided that way without breaking enum names.)
2. **Undeclared variables must be ignored**: `gh issue list` search pagination sends `perPage` that the query does not declare; `gh pr status` always sends both `number` and `headRefName` while each query variant declares only one (see §3 issue/pr sections). Strict servers reject "variable provided but not defined" — ignore extras instead.
3. **`ID` vs `String` variable compatibility** (§4.5): shurcooL declares Go `string` variables as `ID!`.
4. **`"variables": null`** and a missing `variables` key must both be accepted.
5. `__type(name: "X")` for a type the server does not have should return `null` per the GraphQL spec (not an error — any `errors` entry makes the detection call, and therefore the command, fail). Simplest: define every type named in §3.0 (`Issue`, `PullRequest`, `StatusCheckRollupContextConnection`, `WorkflowRun`, `Repository`, `ProjectV2`, `SearchType`, `Release`).

## 5. Consolidated GraphQL schema subset (SDL)

Produced by mechanically merging the per-command SDL fragments (§3) with graphql-core, then checked with `assert_valid_schema`, then **exercised against the real gh 2.102.0 binary** (Appendix A): every GraphQL request gh sent for the commands listed there validated against it except the two leniency cases in §4.6, and gh decoded every response.

### 5.1 How to read this

* This is the **superset** needed by both the GHES path and the github.com/github.localhost path (only the GHES path was exercised against the binary; the github.com-only actor types were derived from source). Fields marked `# FEATURE-GATED` are only queried when introspection (§3.0) or host type says so.
* Field arguments include the full connection argument set (`first/after/last/before`) even where gh uses only some; extra arguments are harmless.
* Nullability mirrors github.com where the fragment authors knew it. gh's decoder tolerates `null` for nullable fields; it fails on missing keys and on extra keys (§4.4).
* Only one conflict arose while merging: `Repository.sshUrl` was `String!` in one fragment and `GitSSHRemote!` in another; github.com uses `GitSSHRemote!` (a string-serialized scalar), which is kept.

### 5.2 Minimal GHES profile (recommended first implementation target)

Serve host as GHES (e.g. `forge.localhost`), `GET /api/v3/meta` → `{"installed_version":"3.20.0"}`, and **omit** these from the schema (so introspection answers "no"):

* `Issue.blockedBy`, `Issue.blocking` (and the `addBlockedBy`/`removeBlockedBy` mutations become unused)
* `PullRequest.isInMergeQueue`, `PullRequest.isMergeQueueEnabled`
* `StatusCheckRollupContextConnection.checkRunCount`, `.checkRunCountsByState`, `.statusContextCount`, `.statusContextCountsByState`
* `WorkflowRun.event`
* `Repository.pullRequestTemplates`, `Repository.autoMergeAllowed`
* `Release.immutable`
* `SearchType.ISSUE_ADVANCED`, `ISSUE_SEMANTIC`, `ISSUE_HYBRID`
* github.com-only actor APIs (never used on GHES): `suggestedActors` (Repository/Issue/PullRequest/Assignable), `PullRequest.suggestedReviewerActors`, `Mutation.replaceActorsForAssignable`, `Mutation.requestReviewsByLogin`

Keep `Repository.visibility`. This exact profile was run in Appendix A (Run 1).

### 5.3 SDL

```graphql
type Query {
  node(id: ID!): Node
  organization(login: String!): Organization
  repository(owner: String!, name: String!, followRenames: Boolean = true): Repository
  repositoryOwner(login: String!): RepositoryOwner
  search(query: String!, type: SearchType!, first: Int, after: String, last: Int, before: String): SearchResultItemConnection!
  viewer: User!
}

type Mutation {
  addBlockedBy(input: AddBlockedByInput!): AddBlockedByPayload
  addComment(input: AddCommentInput!): AddCommentPayload
  addLabelsToLabelable(input: AddLabelsToLabelableInput!): AddLabelsToLabelablePayload
  addProjectV2ItemById(input: AddProjectV2ItemByIdInput!): AddProjectV2ItemByIdPayload
  addPullRequestReview(input: AddPullRequestReviewInput!): AddPullRequestReviewPayload
  addSubIssue(input: AddSubIssueInput!): AddSubIssuePayload
  cloneTemplateRepository(input: CloneTemplateRepositoryInput!): CloneTemplateRepositoryPayload
  closeIssue(input: CloseIssueInput!): CloseIssuePayload
  closePullRequest(input: ClosePullRequestInput!): ClosePullRequestPayload
  createIssue(input: CreateIssueInput!): CreateIssuePayload
  createPullRequest(input: CreatePullRequestInput!): CreatePullRequestPayload
  createRepository(input: CreateRepositoryInput!): CreateRepositoryPayload
  deleteIssueComment(input: DeleteIssueCommentInput!): DeleteIssueCommentPayload
  deleteProjectV2Item(input: DeleteProjectV2ItemInput!): DeleteProjectV2ItemPayload
  disablePullRequestAutoMerge(input: DisablePullRequestAutoMergeInput!): DisablePullRequestAutoMergePayload
  enablePullRequestAutoMerge(input: EnablePullRequestAutoMergeInput!): EnablePullRequestAutoMergePayload
  mergePullRequest(input: MergePullRequestInput!): MergePullRequestPayload
  removeBlockedBy(input: RemoveBlockedByInput!): RemoveBlockedByPayload
  removeLabelsFromLabelable(input: RemoveLabelsFromLabelableInput!): RemoveLabelsFromLabelablePayload
  removeSubIssue(input: RemoveSubIssueInput!): RemoveSubIssuePayload
  reopenIssue(input: ReopenIssueInput!): ReopenIssuePayload
  reopenPullRequest(input: ReopenPullRequestInput!): ReopenPullRequestPayload
  replaceActorsForAssignable(input: ReplaceActorsForAssignableInput!): ReplaceActorsForAssignablePayload  # FEATURE-GATED: github.com/github.localhost only
  requestReviews(input: RequestReviewsInput!): RequestReviewsPayload
  requestReviewsByLogin(input: RequestReviewsByLoginInput!): RequestReviewsByLoginPayload  # FEATURE-GATED: github.com/github.localhost only
  updateIssue(input: UpdateIssueInput!): UpdateIssuePayload
  updateIssueComment(input: UpdateIssueCommentInput!): UpdateIssueCommentPayload
  updateIssueIssueType(input: UpdateIssueIssueTypeInput!): UpdateIssueIssueTypePayload
  updatePullRequest(input: UpdatePullRequestInput!): UpdatePullRequestPayload
  updateRepository(input: UpdateRepositoryInput!): UpdateRepositoryPayload
}

scalar DateTime

scalar GitObjectID

scalar GitSSHRemote

scalar HTML

scalar URI

interface Actor {
  avatarUrl(size: Int): URI!
  login: String!
  resourcePath: URI!
  url: URI!
}

interface Assignable {
  assignedActors(first: Int, after: String, last: Int, before: String): AssigneeConnection!
  assignees(first: Int, after: String, last: Int, before: String): UserConnection!
  suggestedActors(first: Int, after: String, last: Int, before: String, query: String): AssigneeConnection!  # FEATURE-GATED: github.com/github.localhost only
}

interface Closable {
  closed: Boolean!
  closedAt: DateTime
}

interface Comment {
  author: Actor
  authorAssociation: CommentAuthorAssociation!
  body: String!
  createdAt: DateTime!
  id: ID!
  includesCreatedEdit: Boolean!
}

interface GitObject {
  oid: GitObjectID!
}

interface Labelable {
  labels(first: Int, after: String, last: Int, before: String, orderBy: LabelOrder): LabelConnection
}

interface Minimizable {
  isMinimized: Boolean!
  minimizedReason: String
}

interface Node {
  id: ID!
}

interface Reactable {
  reactionGroups: [ReactionGroup!]
}

interface RepositoryOwner {
  id: ID!
  login: String!
  repositories(first: Int, after: String, last: Int, before: String, privacy: RepositoryPrivacy, isFork: Boolean, ownerAffiliations: [RepositoryAffiliation] = [OWNER, COLLABORATOR], affiliations: [RepositoryAffiliation], orderBy: RepositoryOrder): RepositoryConnection!
}

interface UniformResourceLocatable {
  resourcePath: URI!
  url: URI!
}

type ActorConnection {
  nodes: [Actor]
  pageInfo: PageInfo!
  totalCount: Int!
}

type AddBlockedByPayload {
  blockingIssue: Issue
  clientMutationId: String
  issue: Issue
}

type AddCommentPayload {
  clientMutationId: String
  commentEdge: IssueCommentEdge
  subject: Node
}

type AddLabelsToLabelablePayload {
  clientMutationId: String
  labelable: Labelable
}

type AddProjectV2ItemByIdPayload {
  clientMutationId: String
  item: ProjectV2Item
}

type AddPullRequestReviewPayload {
  clientMutationId: String
}

type AddSubIssuePayload {
  clientMutationId: String
  issue: Issue
  subIssue: Issue
}

type AssigneeConnection {
  nodes: [Assignee]
  pageInfo: PageInfo!
  totalCount: Int!
}

type AutoMergeRequest {
  authorEmail: String
  commitBody: String
  commitHeadline: String
  enabledAt: DateTime
  enabledBy: Actor
  mergeMethod: PullRequestMergeMethod!
}

type Bot implements Node & Actor {
  avatarUrl(size: Int): URI!
  id: ID!
  login: String!
  resourcePath: URI!
  url: URI!
}

type BranchProtectionRule {
  requiredApprovingReviewCount: Int
  requiresStrictStatusChecks: Boolean!
}

type CheckRun implements Node {
  checkSuite: CheckSuite!
  completedAt: DateTime
  conclusion: CheckConclusionState
  detailsUrl: URI
  id: ID!
  isRequired(pullRequestId: ID, pullRequestNumber: Int): Boolean!
  name: String!
  startedAt: DateTime
  status: CheckStatusState!
}

type CheckRunStateCount {
  count: Int!
  state: CheckRunState!
}

type CheckSuite {
  workflowRun: WorkflowRun
}

type CloneTemplateRepositoryPayload {
  clientMutationId: String
  repository: Repository
}

type CloseIssuePayload {
  clientMutationId: String
  issue: Issue
}

type ClosePullRequestPayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type CodeOfConduct {
  key: String!
  name: String!
  url: URI
}

type Commit implements Node & GitObject {
  authoredDate: DateTime!
  authors(first: Int, after: String): GitActorConnection!
  committedDate: DateTime!
  id: ID!
  messageBody: String!
  messageHeadline: String!
  oid: GitObjectID!
  statusCheckRollup: StatusCheckRollup
}

type CreateIssuePayload {
  clientMutationId: String
  issue: Issue
}

type CreatePullRequestPayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type CreateRepositoryPayload {
  clientMutationId: String
  repository: Repository
}

type DeleteIssueCommentPayload {
  clientMutationId: String
}

type DeleteProjectV2ItemPayload {
  clientMutationId: String
  deletedItemId: ID
}

type DisablePullRequestAutoMergePayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type EnablePullRequestAutoMergePayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type FundingLink {
  platform: FundingPlatform!
  url: URI!
}

type GitActor {
  email: String
  name: String
  user: User
}

type GitActorConnection {
  nodes: [GitActor]
}

type Issue implements Node & Assignable & Closable & Comment & Labelable & Reactable & UniformResourceLocatable {
  activeLockReason: LockReason
  assignedActors(first: Int, after: String, last: Int, before: String): AssigneeConnection!
  assignees(first: Int, after: String, last: Int, before: String): UserConnection!
  author: Actor
  authorAssociation: CommentAuthorAssociation!
  blockedBy(first: Int, after: String, last: Int, before: String): IssueConnection!  # FEATURE-GATED: GHES: only if Issue_fields reports blockedBy (IssueRelationshipsSupported)
  blocking(first: Int, after: String, last: Int, before: String): IssueConnection!  # FEATURE-GATED: same as blockedBy
  body: String!
  closed: Boolean!
  closedAt: DateTime
  closedByPullRequestsReferences(first: Int, after: String, last: Int, before: String, includeClosedPrs: Boolean = false, orderByState: Boolean = false, userLinkedOnly: Boolean = false): PullRequestConnection
  comments(first: Int, after: String, last: Int, before: String, orderBy: IssueCommentOrder): IssueCommentConnection!
  createdAt: DateTime!
  id: ID!
  includesCreatedEdit: Boolean!
  isPinned: Boolean
  issueType: IssueType
  labels(first: Int, after: String, last: Int, before: String, orderBy: LabelOrder): LabelConnection
  locked: Boolean!
  milestone: Milestone
  number: Int!
  parent: Issue
  projectCards(first: Int, after: String, last: Int, before: String, archivedStates: [ProjectCardArchivedState]): ProjectCardConnection!
  projectItems(first: Int, after: String, last: Int, before: String, includeArchived: Boolean = true): ProjectV2ItemConnection!
  reactionGroups: [ReactionGroup!]
  repository: Repository!
  resourcePath: URI!
  state: IssueState!
  stateReason(enableDuplicate: Boolean = false): IssueStateReason
  subIssues(first: Int, after: String, last: Int, before: String): IssueConnection!
  subIssuesSummary: SubIssuesSummary!
  suggestedActors(first: Int, after: String, last: Int, before: String, query: String): AssigneeConnection!  # FEATURE-GATED: github.com/github.localhost only (ApiActorsSupported)
  title: String!
  updatedAt: DateTime!
  url: URI!
}

type IssueComment implements Node & Comment & Reactable & Minimizable & UniformResourceLocatable {
  author: Actor
  authorAssociation: CommentAuthorAssociation!
  body: String!
  createdAt: DateTime!
  id: ID!
  includesCreatedEdit: Boolean!
  isMinimized: Boolean!
  minimizedReason: String
  reactionGroups: [ReactionGroup!]
  resourcePath: URI!
  url: URI!
  viewerDidAuthor: Boolean!
}

type IssueCommentConnection {
  nodes: [IssueComment]
  pageInfo: PageInfo!
  totalCount: Int!
}

type IssueCommentEdge {
  cursor: String!
  node: IssueComment
}

type IssueConnection {
  nodes: [Issue]
  pageInfo: PageInfo!
  totalCount: Int!
}

type IssueTemplate {
  about: String
  body: String
  name: String!
  title: String
}

type IssueType implements Node {
  color: IssueTypeColor!
  description: String
  id: ID!
  name: String!
}

type IssueTypeConnection {
  nodes: [IssueType]
  pageInfo: PageInfo!
  totalCount: Int!
}

type Label implements Node {
  color: String!
  createdAt: DateTime
  description: String
  id: ID!
  isDefault: Boolean!
  name: String!
  updatedAt: DateTime
  url: URI!
}

type LabelConnection {
  nodes: [Label]
  pageInfo: PageInfo!
  totalCount: Int!
}

type Language {
  name: String!
}

type LanguageConnection {
  edges: [LanguageEdge]
  totalCount: Int!
}

type LanguageEdge {
  node: Language!
  size: Int!
}

type License {
  key: String!
  name: String!
  nickname: String
}

type Mannequin implements Node & Actor {
  avatarUrl(size: Int): URI!
  id: ID!
  login: String!
  resourcePath: URI!
  url: URI!
}

type MergePullRequestPayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type Milestone implements Node {
  description: String
  dueOn: DateTime
  id: ID!
  number: Int!
  state: MilestoneState!
  title: String!
}

type MilestoneConnection {
  nodes: [Milestone]
  pageInfo: PageInfo!
  totalCount: Int!
}

type Organization implements Node & Actor & RepositoryOwner {
  avatarUrl(size: Int): URI!
  id: ID!
  login: String!
  name: String
  projects(first: Int, after: String, last: Int, before: String, states: [ProjectState!], orderBy: ProjectOrder, search: String): ProjectConnection!
  projectsV2(first: Int, after: String, last: Int, before: String, query: String, orderBy: ProjectV2Order): ProjectV2Connection!
  repositories(first: Int, after: String, last: Int, before: String, privacy: RepositoryPrivacy, isFork: Boolean, ownerAffiliations: [RepositoryAffiliation] = [OWNER, COLLABORATOR], affiliations: [RepositoryAffiliation], orderBy: RepositoryOrder): RepositoryConnection!
  resourcePath: URI!
  teams(first: Int, after: String, orderBy: TeamOrder, query: String): TeamConnection!
  url: URI!
}

type OrganizationConnection {
  nodes: [Organization]
  pageInfo: PageInfo!
  totalCount: Int!
}

type PageInfo {
  endCursor: String
  hasNextPage: Boolean!
  hasPreviousPage: Boolean!
  startCursor: String
}

type Project implements Node {
  body: String
  id: ID!
  name: String!
  number: Int!
  resourcePath: URI!
}

type ProjectCard implements Node {
  column: ProjectColumn
  id: ID!
  project: Project!
}

type ProjectCardConnection {
  nodes: [ProjectCard]
  pageInfo: PageInfo!
  totalCount: Int!
}

type ProjectColumn implements Node {
  id: ID!
  name: String!
}

type ProjectConnection {
  nodes: [Project]
  pageInfo: PageInfo!
  totalCount: Int!
}

type ProjectV2 implements Node {
  closed: Boolean!
  id: ID!
  number: Int!
  resourcePath: URI!
  title: String!
  url: URI!
}

type ProjectV2Connection {
  nodes: [ProjectV2]
  pageInfo: PageInfo!
  totalCount: Int!
}

type ProjectV2Item implements Node {
  content: ProjectV2ItemContent
  fieldValueByName(name: String!): ProjectV2ItemFieldValue
  id: ID!
  project: ProjectV2!
}

type ProjectV2ItemConnection {
  nodes: [ProjectV2Item]
  pageInfo: PageInfo!
  totalCount: Int!
}

type ProjectV2ItemFieldSingleSelectValue implements Node {
  id: ID!
  name: String
  optionId: String
}

type PullRequest implements Node & Assignable & Closable & Comment & Labelable & Reactable & UniformResourceLocatable {
  additions: Int!
  assignedActors(first: Int, after: String, last: Int, before: String): AssigneeConnection!
  assignees(first: Int, after: String, last: Int, before: String): UserConnection!
  author: Actor
  authorAssociation: CommentAuthorAssociation!
  autoMergeRequest: AutoMergeRequest
  baseRef: Ref
  baseRefName: String!
  baseRefOid: GitObjectID!
  body: String!
  changedFiles: Int!
  closed: Boolean!
  closedAt: DateTime
  closingIssuesReferences(first: Int, after: String): IssueConnection
  comments(first: Int, after: String, last: Int, before: String, orderBy: IssueCommentOrder): IssueCommentConnection!
  commits(first: Int, after: String, last: Int, before: String): PullRequestCommitConnection!
  createdAt: DateTime!
  deletions: Int!
  files(first: Int, after: String): PullRequestChangedFileConnection
  fullDatabaseId: String
  headRefName: String!
  headRefOid: GitObjectID!
  headRepository: Repository
  headRepositoryOwner: RepositoryOwner
  id: ID!
  includesCreatedEdit: Boolean!
  isCrossRepository: Boolean!
  isDraft: Boolean!
  isInMergeQueue: Boolean!  # FEATURE-GATED: only if PullRequest_fields reports it (MergeQueue)
  isMergeQueueEnabled: Boolean!  # FEATURE-GATED: only with MergeQueue
  labels(first: Int, after: String, last: Int, before: String, orderBy: LabelOrder): LabelConnection
  latestReviews(first: Int, after: String): PullRequestReviewConnection
  maintainerCanModify: Boolean!
  mergeCommit: Commit
  mergeStateStatus: MergeStateStatus!
  mergeable: MergeableState!
  mergedAt: DateTime
  mergedBy: Actor
  milestone: Milestone
  number: Int!
  potentialMergeCommit: Commit
  projectCards(first: Int, after: String, last: Int, before: String): ProjectCardConnection!
  projectItems(first: Int, after: String, last: Int, before: String, includeArchived: Boolean = true): ProjectV2ItemConnection!
  reactionGroups: [ReactionGroup!]
  repository: Repository!
  resourcePath: URI!
  reviewDecision: PullRequestReviewDecision
  reviewRequests(first: Int, after: String): ReviewRequestConnection
  reviews(first: Int, after: String): PullRequestReviewConnection
  state: PullRequestState!
  suggestedActors(first: Int, after: String, last: Int, before: String, query: String): AssigneeConnection!  # FEATURE-GATED: github.com/github.localhost only
  title: String!
  updatedAt: DateTime!
  url: URI!
  viewerMergeBodyText(mergeType: PullRequestMergeMethod): String!
  viewerMergeHeadlineText(mergeType: PullRequestMergeMethod): String!
}

type PullRequestChangedFile {
  additions: Int!
  changeType: PatchStatus!
  deletions: Int!
  path: String!
}

type PullRequestChangedFileConnection {
  nodes: [PullRequestChangedFile]
}

type PullRequestCommit {
  commit: Commit!
}

type PullRequestCommitConnection {
  nodes: [PullRequestCommit]
  totalCount: Int!
}

type PullRequestConnection {
  edges: [PullRequestEdge]
  nodes: [PullRequest]
  pageInfo: PageInfo!
  totalCount: Int!
}

type PullRequestEdge {
  node: PullRequest
}

type PullRequestReview implements Node {
  author: Actor
  authorAssociation: CommentAuthorAssociation!
  body: String!
  commit: Commit
  id: ID!
  includesCreatedEdit: Boolean!
  reactionGroups: [ReactionGroup!]
  state: PullRequestReviewState!
  submittedAt: DateTime
  url: URI!
}

type PullRequestReviewConnection {
  nodes: [PullRequestReview]
  pageInfo: PageInfo!
  totalCount: Int!
}

type PullRequestTemplate {
  body: String
  filename: String
}

type ReactingUserConnection {
  totalCount: Int!
}

type ReactionGroup {
  content: ReactionContent!
  users(first: Int, after: String, last: Int, before: String): ReactingUserConnection!
}

type Ref implements Node {
  branchProtectionRule: BranchProtectionRule
  id: ID!
  name: String!
  target: GitObject
}

type Release implements Node {
  createdAt: DateTime!
  databaseId: Int
  id: ID!
  immutable: Boolean!  # FEATURE-GATED: only if Release_fields reports it
  isDraft: Boolean!
  isLatest: Boolean!
  isPrerelease: Boolean!
  name: String
  publishedAt: DateTime
  tagName: String!
  url: URI!
}

type ReleaseConnection {
  nodes: [Release]
  pageInfo: PageInfo!
  totalCount: Int!
}

type RemoveBlockedByPayload {
  blockingIssue: Issue
  clientMutationId: String
  issue: Issue
}

type RemoveLabelsFromLabelablePayload {
  clientMutationId: String
  labelable: Labelable
}

type RemoveSubIssuePayload {
  clientMutationId: String
  issue: Issue
  subIssue: Issue
}

type ReopenIssuePayload {
  clientMutationId: String
  issue: Issue
}

type ReopenPullRequestPayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type ReplaceActorsForAssignablePayload {
  assignable: Assignable
  clientMutationId: String
}

type Repository implements Node {
  archivedAt: DateTime
  assignableUsers(first: Int, after: String, last: Int, before: String, query: String): UserConnection!
  autoMergeAllowed: Boolean!  # FEATURE-GATED: GHES: only if Repository_fields reports it
  codeOfConduct: CodeOfConduct
  contactLinks: [RepositoryContactLink!]
  createdAt: DateTime!
  databaseId: Int
  defaultBranchRef: Ref
  deleteBranchOnMerge: Boolean!
  description: String
  diskUsage: Int
  forkCount: Int!
  forks(first: Int, after: String, affiliations: [RepositoryAffiliation]): RepositoryConnection!
  fundingLinks: [FundingLink!]!
  hasDiscussionsEnabled: Boolean!
  hasIssuesEnabled: Boolean!
  hasProjectsEnabled: Boolean!
  hasWikiEnabled: Boolean!
  homepageUrl: URI
  id: ID!
  isArchived: Boolean!
  isBlankIssuesEnabled: Boolean!
  isEmpty: Boolean!
  isFork: Boolean!
  isInOrganization: Boolean!
  isMirror: Boolean!
  isPrivate: Boolean!
  isSecurityPolicyEnabled: Boolean
  isTemplate: Boolean!
  isUserConfigurationRepository: Boolean!
  issue(number: Int!): Issue
  issueOrPullRequest(number: Int!): IssueOrPullRequest
  issueTemplates: [IssueTemplate!]
  issueTypes(first: Int, after: String, last: Int, before: String): IssueTypeConnection
  issues(first: Int, after: String, last: Int, before: String, orderBy: IssueOrder, states: [IssueState!], labels: [String!], filterBy: IssueFilters): IssueConnection!
  labels(first: Int, after: String, last: Int, before: String, orderBy: LabelOrder, query: String): LabelConnection
  languages(first: Int, after: String, last: Int, before: String, orderBy: LanguageOrder): LanguageConnection
  latestRelease: Release
  licenseInfo: License
  mentionableUsers(first: Int, after: String, last: Int, before: String, query: String): UserConnection!
  mergeCommitAllowed: Boolean!
  milestone(number: Int!): Milestone
  milestones(first: Int, after: String, last: Int, before: String, states: [MilestoneState!], orderBy: MilestoneOrder, query: String): MilestoneConnection
  mirrorUrl: URI
  name: String!
  nameWithOwner: String!
  openGraphImageUrl: URI!
  owner: RepositoryOwner!
  parent: Repository
  primaryLanguage: Language
  projects(first: Int, after: String, last: Int, before: String, states: [ProjectState!], orderBy: ProjectOrder, search: String): ProjectConnection!
  projectsV2(first: Int, after: String, last: Int, before: String, query: String, orderBy: ProjectV2Order = {field: NUMBER, direction: DESC}): ProjectV2Connection!
  pullRequest(number: Int!): PullRequest
  pullRequestTemplates: [PullRequestTemplate!]  # FEATURE-GATED: GHES: only if Repository_fields reports it
  pullRequests(states: [PullRequestState!], labels: [String!], headRefName: String, baseRefName: String, orderBy: IssueOrder, first: Int, after: String, last: Int, before: String): PullRequestConnection!
  pushedAt: DateTime
  rebaseMergeAllowed: Boolean!
  ref(qualifiedName: String!): Ref
  release(tagName: String!): Release
  releases(first: Int, after: String, orderBy: ReleaseOrder): ReleaseConnection!
  repositoryTopics(first: Int, after: String, last: Int, before: String): RepositoryTopicConnection!
  securityPolicyUrl: URI
  squashMergeAllowed: Boolean!
  sshUrl: GitSSHRemote!
  stargazerCount: Int!
  suggestedActors(first: Int, after: String, last: Int, before: String, query: String, loginNames: String, capabilities: [RepositorySuggestedActorFilter!]!): ActorConnection!  # FEATURE-GATED: github.com/github.localhost only
  templateRepository: Repository
  updatedAt: DateTime!
  url: URI!
  usesCustomOpenGraphImage: Boolean!
  viewerCanAdminister: Boolean!
  viewerDefaultCommitEmail: String
  viewerDefaultMergeMethod: PullRequestMergeMethod!
  viewerHasStarred: Boolean!
  viewerPermission: RepositoryPermission
  viewerPossibleCommitEmails: [String!]
  viewerSubscription: SubscriptionState
  visibility: RepositoryVisibility!
  watchers(first: Int, after: String, last: Int, before: String): UserConnection!
}

type RepositoryConnection {
  nodes: [Repository]
  pageInfo: PageInfo!
  totalCount: Int!
}

type RepositoryContactLink {
  about: String!
  name: String!
  url: URI!
}

type RepositoryTopic {
  topic: Topic!
}

type RepositoryTopicConnection {
  nodes: [RepositoryTopic]
}

type RequestReviewsByLoginPayload {
  clientMutationId: String
}

type RequestReviewsPayload {
  clientMutationId: String
}

type ReviewRequest {
  requestedReviewer: RequestedReviewer
}

type ReviewRequestConnection {
  nodes: [ReviewRequest]
  totalCount: Int!
}

type SearchResultItemConnection {
  edges: [SearchResultItemEdge]
  issueCount: Int!
  nodes: [SearchResultItem]
  pageInfo: PageInfo!
  repositoryCount: Int!
}

type SearchResultItemEdge {
  node: SearchResultItem
}

type StatusCheckRollup {
  contexts(first: Int, after: String): StatusCheckRollupContextConnection!
}

type StatusCheckRollupContextConnection {
  checkRunCount: Int!  # FEATURE-GATED: only if introspection reports it
  checkRunCountsByState: [CheckRunStateCount!]  # FEATURE-GATED: only with checkRunCount
  nodes: [StatusCheckRollupContext]
  pageInfo: PageInfo!
  statusContextCount: Int!  # FEATURE-GATED: only with checkRunCount
  statusContextCountsByState: [StatusContextStateCount!]  # FEATURE-GATED: only with checkRunCount
}

type StatusContext implements Node {
  context: String!
  createdAt: DateTime!
  description: String
  id: ID!
  isRequired(pullRequestId: ID, pullRequestNumber: Int): Boolean!
  state: StatusState!
  targetUrl: URI
}

type StatusContextStateCount {
  count: Int!
  state: StatusState!
}

type SubIssuesSummary {
  completed: Int!
  percentCompleted: Int!
  total: Int!
}

type Team implements Node {
  id: ID!
  name: String!
  organization: Organization!
  slug: String!
}

type TeamConnection {
  nodes: [Team]
  pageInfo: PageInfo!
}

type Topic {
  name: String!
}

type UpdateIssueCommentPayload {
  clientMutationId: String
  issueComment: IssueComment
}

type UpdateIssueIssueTypePayload {
  clientMutationId: String
  issue: Issue
}

type UpdateIssuePayload {
  clientMutationId: String
  issue: Issue
}

type UpdatePullRequestPayload {
  clientMutationId: String
  pullRequest: PullRequest
}

type UpdateRepositoryPayload {
  clientMutationId: String
  repository: Repository
}

type User implements Node & Actor & RepositoryOwner {
  avatarUrl(size: Int): URI!
  databaseId: Int
  id: ID!
  login: String!
  name: String
  organizations(first: Int, after: String, last: Int, before: String): OrganizationConnection!
  projectsV2(first: Int, after: String, last: Int, before: String, query: String, orderBy: ProjectV2Order): ProjectV2Connection!
  repositories(first: Int, after: String, last: Int, before: String, privacy: RepositoryPrivacy, isFork: Boolean, ownerAffiliations: [RepositoryAffiliation] = [OWNER, COLLABORATOR], affiliations: [RepositoryAffiliation], orderBy: RepositoryOrder): RepositoryConnection!
  resourcePath: URI!
  url: URI!
}

type UserConnection {
  nodes: [User]
  pageInfo: PageInfo!
  totalCount: Int!
}

type Workflow {
  name: String!
}

type WorkflowRun {
  event: String!  # FEATURE-GATED: only if PullRequest_fields2 reports it
  workflow: Workflow!
}

union Assignee = Bot | Mannequin | Organization | User

union IssueOrPullRequest = Issue | PullRequest

union ProjectV2ItemContent = Issue | PullRequest

union ProjectV2ItemFieldValue = ProjectV2ItemFieldSingleSelectValue

union RequestedReviewer = User | Team | Bot | Mannequin

union SearchResultItem = Issue | PullRequest | Repository | User | Organization

union StatusCheckRollupContext = CheckRun | StatusContext

enum CheckConclusionState {
  ACTION_REQUIRED
  TIMED_OUT
  CANCELLED
  FAILURE
  SUCCESS
  NEUTRAL
  SKIPPED
  STARTUP_FAILURE
  STALE
}

enum CheckRunState {
  ACTION_REQUIRED
  CANCELLED
  COMPLETED
  FAILURE
  IN_PROGRESS
  NEUTRAL
  PENDING
  QUEUED
  SKIPPED
  STALE
  STARTUP_FAILURE
  SUCCESS
  TIMED_OUT
  WAITING
}

enum CheckStatusState {
  REQUESTED
  QUEUED
  IN_PROGRESS
  COMPLETED
  WAITING
  PENDING
}

enum CommentAuthorAssociation {
  MEMBER
  OWNER
  MANNEQUIN
  COLLABORATOR
  CONTRIBUTOR
  FIRST_TIME_CONTRIBUTOR
  FIRST_TIMER
  NONE
}

enum DiffSide {
  LEFT
  RIGHT
}

enum FundingPlatform {
  GITHUB
  PATREON
  OPEN_COLLECTIVE
  KO_FI
  TIDELIFT
  COMMUNITY_BRIDGE
  LIBERAPAY
  ISSUEHUNT
  LFX_CROWDFUNDING
  POLAR
  BUY_ME_A_COFFEE
  THANKS_DEV
  CUSTOM
}

enum IssueClosedStateReason {
  COMPLETED
  NOT_PLANNED
  DUPLICATE
}

enum IssueCommentOrderField {
  UPDATED_AT
}

enum IssueOrderField {
  CREATED_AT
  UPDATED_AT
  COMMENTS
}

enum IssueState {
  OPEN
  CLOSED
}

enum IssueStateReason {
  REOPENED
  NOT_PLANNED
  COMPLETED
  DUPLICATE
}

enum IssueTypeColor {
  GRAY
  BLUE
  GREEN
  YELLOW
  ORANGE
  RED
  PINK
  PURPLE
}

enum LabelOrderField {
  NAME
  CREATED_AT
}

enum LanguageOrderField {
  SIZE
}

enum LockReason {
  OFF_TOPIC
  TOO_HEATED
  RESOLVED
  SPAM
}

enum MergeStateStatus {
  BEHIND
  BLOCKED
  CLEAN
  DIRTY
  DRAFT
  HAS_HOOKS
  UNKNOWN
  UNSTABLE
}

enum MergeableState {
  MERGEABLE
  CONFLICTING
  UNKNOWN
}

enum MilestoneOrderField {
  CREATED_AT
  UPDATED_AT
  DUE_DATE
  NUMBER
}

enum MilestoneState {
  OPEN
  CLOSED
}

enum OrderDirection {
  ASC
  DESC
}

enum PatchStatus {
  ADDED
  DELETED
  RENAMED
  COPIED
  MODIFIED
  CHANGED
}

enum ProjectCardArchivedState {
  ARCHIVED
  NOT_ARCHIVED
}

enum ProjectOrderField {
  CREATED_AT
  UPDATED_AT
  NAME
}

enum ProjectState {
  OPEN
  CLOSED
}

enum ProjectV2OrderField {
  TITLE
  NUMBER
  UPDATED_AT
  CREATED_AT
}

enum PullRequestMergeMethod {
  MERGE
  SQUASH
  REBASE
}

enum PullRequestReviewDecision {
  CHANGES_REQUESTED
  APPROVED
  REVIEW_REQUIRED
}

enum PullRequestReviewEvent {
  COMMENT
  APPROVE
  REQUEST_CHANGES
  DISMISS
}

enum PullRequestReviewState {
  PENDING
  COMMENTED
  APPROVED
  CHANGES_REQUESTED
  DISMISSED
}

enum PullRequestState {
  OPEN
  CLOSED
  MERGED
}

enum PullRequestUpdateState {
  OPEN
  CLOSED
}

enum ReactionContent {
  THUMBS_UP
  THUMBS_DOWN
  LAUGH
  HOORAY
  CONFUSED
  HEART
  ROCKET
  EYES
}

enum ReleaseOrderField {
  CREATED_AT
  NAME
}

enum RepositoryAffiliation {
  OWNER
  COLLABORATOR
  ORGANIZATION_MEMBER
}

enum RepositoryOrderField {
  CREATED_AT
  UPDATED_AT
  PUSHED_AT
  NAME
  STARGAZERS
}

enum RepositoryPermission {
  ADMIN
  MAINTAIN
  WRITE
  TRIAGE
  READ
}

enum RepositoryPrivacy {
  PUBLIC
  PRIVATE
}

enum RepositorySuggestedActorFilter {
  CAN_BE_ASSIGNED
  CAN_BE_AUTHOR
}

enum RepositoryVisibility {
  PRIVATE
  PUBLIC
  INTERNAL
}

enum SearchType {
  ISSUE
  REPOSITORY
  USER
  DISCUSSION
  ISSUE_ADVANCED  # FEATURE-GATED: presence forces type: ISSUE_ADVANCED opt-in; omit (recommended)
}

enum StatusState {
  EXPECTED
  ERROR
  FAILURE
  PENDING
  SUCCESS
}

enum SubscriptionState {
  UNSUBSCRIBED
  SUBSCRIBED
  IGNORED
}

enum TeamOrderField {
  NAME
}

input AddBlockedByInput {
  blockingIssueId: ID!
  clientMutationId: String
  issueId: ID!
}

input AddCommentInput {
  body: String!
  clientMutationId: String
  subjectId: ID!
}

input AddLabelsToLabelableInput {
  clientMutationId: String
  labelIds: [ID!]!
  labelableId: ID!
}

input AddProjectV2ItemByIdInput {
  clientMutationId: String
  contentId: ID!
  projectId: ID!
}

input AddPullRequestReviewInput {
  body: String
  clientMutationId: String
  comments: [DraftPullRequestReviewComment]
  commitOID: GitObjectID
  event: PullRequestReviewEvent
  pullRequestId: ID!
  threads: [DraftPullRequestReviewThread]
}

input AddSubIssueInput {
  clientMutationId: String
  issueId: ID!
  replaceParent: Boolean
  subIssueId: ID
  subIssueUrl: String
}

input CloneTemplateRepositoryInput {
  clientMutationId: String
  description: String
  includeAllBranches: Boolean = false
  name: String!
  ownerId: ID!
  repositoryId: ID!
  visibility: RepositoryVisibility!
}

input CloseIssueInput {
  clientMutationId: String
  duplicateIssueId: ID
  issueId: ID!
  stateReason: IssueClosedStateReason
}

input ClosePullRequestInput {
  clientMutationId: String
  pullRequestId: ID!
}

input CreateIssueInput {
  assigneeIds: [ID!]
  body: String
  clientMutationId: String
  issueTemplate: String
  issueTypeId: ID
  labelIds: [ID!]
  milestoneId: ID
  parentIssueId: ID
  projectIds: [ID!]
  repositoryId: ID!
  title: String!
}

input CreatePullRequestInput {
  baseRefName: String!
  body: String
  clientMutationId: String
  draft: Boolean
  headRefName: String!
  headRepositoryId: ID
  maintainerCanModify: Boolean
  repositoryId: ID!
  title: String!
}

input CreateRepositoryInput {
  clientMutationId: String
  description: String
  hasIssuesEnabled: Boolean
  hasWikiEnabled: Boolean
  homepageUrl: URI
  name: String!
  ownerId: ID
  teamId: ID
  template: Boolean
  visibility: RepositoryVisibility!
}

input DeleteIssueCommentInput {
  clientMutationId: String
  id: ID!
}

input DeleteProjectV2ItemInput {
  clientMutationId: String
  itemId: ID!
  projectId: ID!
}

input DisablePullRequestAutoMergeInput {
  clientMutationId: String
  pullRequestId: ID!
}

input DraftPullRequestReviewComment {
  body: String!
  path: String!
  position: Int!
}

input DraftPullRequestReviewThread {
  body: String!
  line: Int!
  path: String!
  side: DiffSide
  startLine: Int
  startSide: DiffSide
}

input EnablePullRequestAutoMergeInput {
  authorEmail: String
  clientMutationId: String
  commitBody: String
  commitHeadline: String
  expectedHeadOid: GitObjectID
  mergeMethod: PullRequestMergeMethod
  pullRequestId: ID!
}

input IssueCommentOrder {
  direction: OrderDirection!
  field: IssueCommentOrderField!
}

input IssueFilters {
  assignee: String
  createdBy: String
  labels: [String!]
  mentioned: String
  milestone: String
  milestoneNumber: String
  since: DateTime
  states: [IssueState!]
  type: String
  viewerSubscribed: Boolean = false
}

input IssueOrder {
  direction: OrderDirection!
  field: IssueOrderField!
}

input LabelOrder {
  direction: OrderDirection!
  field: LabelOrderField!
}

input LanguageOrder {
  direction: OrderDirection!
  field: LanguageOrderField!
}

input MergePullRequestInput {
  authorEmail: String
  clientMutationId: String
  commitBody: String
  commitHeadline: String
  expectedHeadOid: GitObjectID
  mergeMethod: PullRequestMergeMethod
  pullRequestId: ID!
}

input MilestoneOrder {
  direction: OrderDirection!
  field: MilestoneOrderField!
}

input ProjectOrder {
  direction: OrderDirection!
  field: ProjectOrderField!
}

input ProjectV2Order {
  direction: OrderDirection!
  field: ProjectV2OrderField!
}

input ReleaseOrder {
  direction: OrderDirection!
  field: ReleaseOrderField!
}

input RemoveBlockedByInput {
  blockingIssueId: ID!
  clientMutationId: String
  issueId: ID!
}

input RemoveLabelsFromLabelableInput {
  clientMutationId: String
  labelIds: [ID!]!
  labelableId: ID!
}

input RemoveSubIssueInput {
  clientMutationId: String
  issueId: ID!
  subIssueId: ID!
}

input ReopenIssueInput {
  clientMutationId: String
  issueId: ID!
}

input ReopenPullRequestInput {
  clientMutationId: String
  pullRequestId: ID!
}

input ReplaceActorsForAssignableInput {
  actorIds: [ID!]
  actorLogins: [String!]
  assignableId: ID!
  clientMutationId: String
}

input RepositoryOrder {
  direction: OrderDirection!
  field: RepositoryOrderField!
}

input RequestReviewsByLoginInput {
  botLogins: [String!]
  clientMutationId: String
  pullRequestId: ID!
  teamSlugs: [String!]
  union: Boolean!
  userLogins: [String!]
}

input RequestReviewsInput {
  clientMutationId: String
  pullRequestId: ID!
  teamIds: [ID!]
  union: Boolean
  userIds: [ID!]
}

input TeamOrder {
  direction: OrderDirection!
  field: TeamOrderField!
}

input UpdateIssueCommentInput {
  body: String!
  clientMutationId: String
  id: ID!
}

input UpdateIssueInput {
  assigneeIds: [ID!]
  body: String
  clientMutationId: String
  id: ID!
  issueTypeId: ID
  labelIds: [ID!]
  milestoneId: ID
  projectIds: [ID!]
  state: IssueState
  title: String
}

input UpdateIssueIssueTypeInput {
  clientMutationId: String
  issueId: ID!
  issueTypeId: ID
}

input UpdatePullRequestInput {
  assigneeIds: [ID!]
  baseRefName: String
  body: String
  clientMutationId: String
  labelIds: [ID!]
  maintainerCanModify: Boolean
  milestoneId: ID
  projectIds: [ID!]
  pullRequestId: ID!
  state: PullRequestUpdateState
  title: String
}

input UpdateRepositoryInput {
  clientMutationId: String
  description: String
  hasDiscussionsEnabled: Boolean
  hasIssuesEnabled: Boolean
  hasProjectsEnabled: Boolean
  hasWikiEnabled: Boolean
  homepageUrl: URI
  name: String
  repositoryId: ID!
  template: Boolean
}
```

## 6. Consolidated REST endpoint list

Paths are relative to the REST base (GHES `https://HOST/api/v3/`, github.localhost `http://api.github.localhost/`). Path segments are `url.PathEscape`d by gh (`internal/safeurl`), so a `/` inside a branch, tag or label name arrives as `%2F`; the server must decode per segment *after* routing (e.g. `git/refs/heads%2Ffeature%2Fx`). All JSON responses need `Content-Type: application/json; charset=utf-8`.

| Method & path | Used by | Request | Response fields read / status semantics |
|---|---|---|---|
| `GET ` (REST root, i.e. `/api/v3/`) | `auth login --with-token`, `auth status` | `Authorization: token T` | **must be 200**; header `X-OAuth-Scopes` (absent ⇒ no scope check; present ⇒ needs `repo` and `read:org`/`write:org`/`admin:org`) |
| `GET meta` | GHES only: search, Projects-v1 and Actions feature detection | — | `installed_version` (semver string; missing ⇒ `issue/pr list --search`/filters fail). Recommend `"3.20.0"` |
| `GET users/{username}` | `repo create OWNER/NAME` | — | `node_id`, `type` (`"User"`/`"Organization"`); 404 ⇒ error |
| `GET orgs/{org}/teams/{team_slug}` | `repo create --team` | — | `id`, `node_id`, `organization.node_id` |
| `POST user/repos` | `repo create` with `--gitignore`/`--license`/`--add-readme` (personal) | `{"name","homepage"?,"description"?,"private","visibility"?,"team_id"?,"has_issues","has_wiki","gitignore_template"?,"license_template"?,"auto_init"?}` (without those three flags `repo create` uses the GraphQL `createRepository` mutation instead) | `node_id`, `name`, `owner.login`, `html_url`, `private`, `created_at` (201) |
| `POST orgs/{org}/repos` | same, org owner | same + `visibility` | same |
| `GET gitignore/templates`, `GET licenses` | interactive `repo create` only | — | names / `[{key,name}]` |
| `GET repos/{owner}/{repo}/readme[?ref=B]` | `repo view` | — | `name`, `content` (base64), `html_url`; **404 ⇒ "no README"** (not an error) |
| `POST repos/{owner}/{repo}/forks` | `repo fork`, interactive `pr create` fork | `{organization?, name?, default_branch_only?}` | `node_id`, `name`, `owner.login`, `created_at` (RFC 3339; older than 1 min ⇒ "already exists"); any 2xx (GitHub: 202). Fork must be git-clonable within ~6 s |
| `PATCH repos/{owner}/{repo}` | `repo fork --fork-name` rename | `{"name": F}` | repo object |
| `DELETE repos/{owner}/{repo}` | `repo delete --yes` | — | 204. Redirects are **not followed**; 301/307/308 ⇒ "renamed or transferred" error. On 403/404 with `X-OAuth-Scopes` lacking `delete_repo`, gh suggests the scope |
| `POST repos/{owner}/{repo}/labels` | `label create` | `{"color","description","name"}` | 201 label; **422 with exactly one error `{"field":"name","code":"already_exists"}`** ⇒ "already exists" (with `--force` ⇒ PATCH) |
| `PATCH repos/{owner}/{repo}/labels/{name}` | `label create --force` on existing | `{"color"?, "description"?}` (+`new_name` not used) | label |
| `GET repos/{owner}/{repo}/pulls/{number}` with `Accept: application/vnd.github.v3.diff` (or `.v3.patch` for `--patch`) | `pr diff` | — | **200 with raw diff/patch text body** |
| `POST repos/{owner}/{repo}/pulls/{number}/requested_reviewers` | `pr edit --add-reviewer` (GHES path; observed) | `{"reviewers":[...],"team_reviewers":[...]}` | — |
| `DELETE repos/{owner}/{repo}/git/refs/heads%2F{branch}` | `pr merge --delete-branch` (404/422 tolerated), `pr close --delete-branch` (any non-2xx is an error) | — | 204 |
| `GET repos/{owner}/{repo}/tags?per_page=5` | interactive `release create` | — | `[{name}]` |
| `POST repos/{owner}/{repo}/releases/generate-notes` | `release create --generate-notes` / interactive | `{tag_name, target_commitish?, previous_tag_name?}` | `{name, body}`; 404 ⇒ treated as unsupported |
| `HEAD repos/{owner}/{repo}/releases/tags/{tag}` | `release create` with assets and not draft | — | 200 ⇒ abort ("release already exists"), 404 ⇒ proceed |
| `POST repos/{owner}/{repo}/releases` | `release create` | `{tag_name, draft, prerelease, name?, body?, target_commitish?, make_latest?: "true"\|"false" (string), discussion_category_name?, generate_release_notes?}` | 201 Release (see below) |
| `POST <upload_url up to first "{">?label=<L>&name=<N>` | `release create` with assets | raw bytes, `Content-Type` by extension, `Content-Length` | ReleaseAsset; retried ×3 on 5xx. `upload_url` **must** contain an RFC 6570 `{?name,label}` suffix. On GHES conventionally `https://HOST/api/uploads/repos/{o}/{r}/releases/{id}/assets{?name,label}` |
| `PATCH <release.url>` | publish after uploads | `{draft:false, discussion_category_name?, make_latest?}` | Release |
| `DELETE <release.url>` | cleanup when upload fails | — | 204 |
| `GET repos/{owner}/{repo}/releases/latest` | `release view` (no tag) | — | Release; 404 ⇒ "release not found" |
| `GET repos/{owner}/{repo}/releases/tags/{tag}` | `release view TAG` (runs concurrently with GraphQL `RepositoryReleaseByTag`) | — | Release; 404 ⇒ fall back to GraphQL `isDraft`/`databaseId` |
| `GET repos/{owner}/{repo}/releases/{databaseId}` | `release view TAG` for a draft | — | Release |
| `GET repos/{owner}/{repo}/compare/{tag}...HEAD?per_page=1` | `release create --fail-on-no-commits` | — | `status` (`"ahead"`) |
| arbitrary | `gh api` | see §3 gh api | pagination via `Link: <url>; rel="next"` |

**Release JSON** read by gh: `id` (int64), `node_id`, `tag_name`, `name`, `body`, `draft`, `prerelease`, `immutable`, `created_at`, `published_at` (nullable), `target_commitish`, `url` (API URL of the release), `upload_url`, `tarball_url`, `zipball_url`, `html_url`, `author{node_id,login}`, `assets[]{node_id,name,label,size,digest,state,url,created_at,updated_at,download_count,content_type,browser_download_url}`.

**Git smart-HTTP** (git subprocess, not via gh's HTTP client or unix socket): `GET /{owner}/{repo}.git/info/refs?service=git-upload-pack` + `POST /{owner}/{repo}.git/git-upload-pack` (clone, fetch, `pr checkout` needs `refs/pull/{N}/head` advertised/fetchable and `refs/heads/{head}`), `GET …/info/refs?service=git-receive-pack` + `POST …/git-receive-pack` (push from `repo create --source --push`, interactive `pr create`). URL base: `https://HOST/` (GHES) or `http://github.localhost/`. Auth: HTTP Basic, username = login or `x-access-token`, password = token; answer unauthenticated requests to private repos with `401` + `WWW-Authenticate: Basic realm="forge"`.

## Appendix A. Empirical verification against the real gh binary

Method: the merged SDL from §5 was loaded into a graphql-core (Python) mock server listening on a unix socket; gh 2.102.0 was pointed at it with `http_unix_socket` and host `forge.localhost` (GHES path), fresh `XDG_CACHE_HOME` (gh caches feature-detection answers for 24h in its cache dir). The mock fills every selected field with type-correct dummy values, validates each query against the schema, and logs every request. REST endpoints other than `/api/v3/`, `/api/v3/meta` returned 404, so REST-dependent commands stop at the first REST call. Two runs:

* **Run 1 — GHES-minimal schema** (§5 superset minus the feature-gated fields listed in §5.2): every command below completed its GraphQL phase with gh decoding all responses successfully; i.e. the minimal schema is sufficient for these commands on the GHES path.
* **Run 2 — full superset schema** (all introspection answers positive).

⚠ = query is **invalid under strict GraphQL validation** (graphql-js/graphql-core `OverlappingFieldsCanBeMerged`) though GitHub accepts it — see §4.6.

Request sequences observed (feature-detection queries appear only on the first command that needs them because gh caches them):

**Run 1 (GHES-minimal)**

| command | requests in order |
|---|---|
| `gh repo view forge.localhost/o/r` | `RepositoryInfo` → REST `GET /api/v3/repos/o/r/readme` |
| `gh repo list o` | `Repository_fields` → `RepositoryList` |
| `gh label list -R forge.localhost/o/r` | `LabelList` |
| `gh issue list -R forge.localhost/o/r` | `IssueList` |
| `gh issue list -R forge.localhost/o/r --search foo --label bug` | REST `GET /api/v3/meta` → `SearchType_enumValues` → `IssueSearch` |
| `gh issue view 1 -R forge.localhost/o/r --comments` | `Issue_fields` → `IssueByNumber` ⚠ → `IssueProjectItems` |
| `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x` | `IssueRepositoryInfo` → `RepositoryMilestoneList` → `RepositoryLabelList` → `RepositoryAssignableUsers` → `IssueCreate` |
| `gh issue edit 1 -R forge.localhost/o/r --add-assignee alice` | `IssueByNumber` → `RepositoryAssignableUsers` → `IssueUpdate` |
| `gh issue status -R forge.localhost/o/r` | `UserCurrent` → `IssueStatus` |
| `gh release list -R forge.localhost/o/r` | `Release_fields` → `RepositoryReleaseList` |
| `gh release view v1 -R forge.localhost/o/r` | `RepositoryReleaseByTag` → REST `GET /api/v3/repos/o/r/releases/tags/v1` |
| `gh pr list` | `PullRequestList` |
| `gh pr list --author alice` | `PullRequestSearch` |
| `gh pr view 1 --comments` | `PullRequestByNumber` ⚠ → `PullRequestProjectItems` |
| `gh pr status` | `PullRequest_fields2` → `PullRequest_fields` → `UserCurrent` → `PullRequestStatus` |
| `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r` | `RepositoryInfo` → `PullRequestForBranch` → `RepositoryLabelList` → `RepositoryAssignableUsers` → `UserCurrent` → `PullRequestCreate` → `PullRequestCreateMetadata` → `PullRequestCreateRequestReviews` |
| `gh pr merge 1 --merge` | `PullRequestByNumber` |
| `gh pr review 1 --comment -b c` | `PullRequestByNumber` → `PullRequestReviewAdd` |
| `gh pr checks 1` | `PullRequestByNumber` → `PullRequestStatusChecks` |
| `gh pr edit 1 --add-reviewer alice` | `PullRequestByNumber` ⚠ → `PullRequestProjectItems` → REST `POST /api/v3/repos/o/r/pulls/1/requested_reviewers` → `PullRequestUpdate` |

**Run 2 (full superset)**

| command | requests in order |
|---|---|
| `gh issue list -R forge.localhost/o/r` | `IssueList` |
| `gh issue list -R forge.localhost/o/r --state closed --label bug --author alice --assignee alice --milestone v1` | `IssueSearch` |
| `gh issue list -R forge.localhost/o/r --search foo` | `IssueSearch` |
| `gh issue list -R forge.localhost/o/r --json number,title,labels,assignees,author,state,createdAt,url,comments,milestone,body,closed,closedAt,updatedAt,id,reactionGroups,projectItems,stateReason` | `IssueList` |
| `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x` | `IssueRepositoryInfo` → `RepositoryAssignableUsers` → `RepositoryMilestoneList` → `RepositoryLabelList` → `IssueCreate` |
| `gh issue status -R forge.localhost/o/r` | `UserCurrent` → `IssueStatus` |
| `gh issue edit 1 -R forge.localhost/o/r --title N --add-label r --remove-label r --add-assignee alice --milestone x` | `IssueByNumber` → `RepositoryMilestoneList` → `RepositoryLabelList` → `RepositoryAssignableUsers` → `LabelAdd` → `LabelRemove` → `IssueUpdate` |
| `gh pr list` | `PullRequestList` |
| `gh pr list --state merged --base main --head feature --label r --author alice` | `PullRequestSearch` |
| `gh pr list --json number,title,headRefName,state,author,labels,isDraft,url,reviewDecision,statusCheckRollup,mergeable,files,commits,reviews,latestReviews,reviewRequests,assignees,milestone,comments,closingIssuesReferences,additions,deletions,autoMergeRequest,mergeCommit,potentialMergeCommit,baseRefName,body,id,changedFiles` | `PullRequestList` ⚠ |
| `gh pr view 1` | `PullRequestByNumber` ⚠ → `PullRequestProjectItems` |
| `gh pr view 1 --comments` | `PullRequestByNumber` ⚠ → `PullRequestProjectItems` |
| `gh pr view` | `PullRequestForBranch` ⚠ |
| `gh pr status` | `PullRequest_fields` → `PullRequest_fields2` → `UserCurrent` → `PullRequestStatus` |
| `gh pr diff 1` | `PullRequestByNumber` → REST `GET /api/v3/repos/o/r/pulls/1` (Accept: `application/vnd.github.v3.diff`) |
| `gh pr checkout 1` | `PullRequestByNumber` |
| `gh pr create -t T -b B --head feature --base main` | `RepositoryInfo` → `PullRequestForBranch` → `PullRequestCreate` |
| `gh pr merge 1 --squash --delete-branch` | `PullRequestByNumber` |
| `gh pr merge 1 --rebase` | `PullRequestByNumber` |
| `gh pr close 1 -c bye` | `PullRequestByNumber` → `CommentCreate` → `PullRequestClose` |
| `gh pr review 1 --approve` | `PullRequestByNumber` → `PullRequestReviewAdd` |
| `gh pr review 1 -r -b no` | `PullRequestByNumber` → `PullRequestReviewAdd` |
| `gh pr checks 1` | `PullRequestByNumber` → `PullRequestStatusChecks` |

## Appendix B. Verbatim GraphQL requests captured from gh 2.102.0

Captured on the wire (unix-socket mock server, host `forge.localhost`, GHES path) — see Appendix A for method. Text is exactly as sent (JSON-decoded `query`; whitespace preserved). Variables show the shape only (mock values). Up to two variants per operation are shown.

### `RepositoryInfo` — from `gh repo view forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
query RepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {name,owner{id,login},description}
	}
```
variables: `{"name": "r", "owner": "o"}`

### `Repository_fields` — from `gh repo list o`  
Profile: GHES-minimal schema
```graphql
query Repository_fields{Repository: __type(name: "Repository"){fields(includeDeprecated: true){name}}}
```
variables: `null`

### `RepositoryList` — from `gh repo list o`  
Profile: GHES-minimal schema
```graphql
query RepositoryList($perPage:Int!,$endCursor:String,$privacy:RepositoryPrivacy,$fork:Boolean,$owner:String!) {
		repositoryOwner(login: $owner) {
			login
			repositories(first: $perPage, after: $endCursor, privacy: $privacy, isFork: $fork, ownerAffiliations: OWNER, orderBy: { field: PUSHED_AT, direction: DESC }) {
				nodes{nameWithOwner,description,isPrivate,isFork,isArchived,createdAt,pushedAt,visibility}
				totalCount
				pageInfo{hasNextPage,endCursor}
			}
		}
	}
```
variables: `{"owner": "o", "perPage": 30}`

### `LabelList` — from `gh label list -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
fragment label on Label{color,description,name}
	query LabelList($owner: String!, $repo: String!, $limit: Int!, $endCursor: String, $query: String, $orderBy: LabelOrder) {
		repository(owner: $owner, name: $repo) {
			labels(first: $limit, after: $endCursor, query: $query, orderBy: $orderBy) {
				totalCount,
				nodes {
					...label
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}
```
variables: `{"limit": 30, "orderBy": {"direction": "ASC", "field": "CREATED_AT"}, "owner": "o", "query": "", "repo": "r"}`

### `IssueList` — from `gh issue list -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
fragment issue on Issue {number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount},stateReason}
	query IssueList($owner: String!, $repo: String!, $limit: Int, $endCursor: String, $states: [IssueState!] = OPEN, $assignee: String, $author: String, $mention: String) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issues(first: $limit, after: $endCursor, orderBy: {field: CREATED_AT, direction: DESC}, states: $states, filterBy: {assignee: $assignee, createdBy: $author, mentioned: $mention}) {
				totalCount
				nodes {
					...issue
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}
	
```
variables: `{"limit": 30, "owner": "o", "repo": "r", "states": ["OPEN"]}`

### `SearchType_enumValues` — from `gh issue list -R forge.localhost/o/r --search foo --label bug`  
Profile: GHES-minimal schema
```graphql
query SearchType_enumValues{SearchType: __type(name: "SearchType"){enumValues(includeDeprecated: true){name}}}
```
variables: `null`

### `IssueSearch` — from `gh issue list -R forge.localhost/o/r --search foo --label bug`  
Profile: GHES-minimal schema
```graphql
fragment issue on Issue {number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount},stateReason}query IssueSearch($repo: String!, $owner: String!, $type: SearchType!, $limit: Int, $after: String, $query: String!) {
			repository(name: $repo, owner: $owner) {
				hasIssuesEnabled
			}
			search(type: $type, last: $limit, after: $after, query: $query) {
				issueCount
				nodes { ...issue }
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
```
variables: `{"limit": 30, "owner": "o", "query": "( foo ) label:bug repo:o/r state:open type:issue", "repo": "r", "type": "ISSUE"}`

### `Issue_fields` — from `gh issue view 1 -R forge.localhost/o/r --comments`  
Profile: GHES-minimal schema
```graphql
query Issue_fields{Issue: __type(name: "Issue"){fields(includeDeprecated: true){name}}}
```
variables: `null`

### `IssueByNumber` — from `gh issue view 1 -R forge.localhost/o/r --comments`  
Profile: GHES-minimal schema
```graphql
	query IssueByNumber($owner: String!, $repo: String!, $number: Int!) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issue: issueOrPullRequest(number: $number) {
				__typename
				...on Issue{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},stateReason,issueType{id,name,description,color},parent{id,number,title,url,state,repository{nameWithOwner}},subIssues(first:100){nodes{id,number,title,url,state,repository{nameWithOwner}},totalCount},subIssuesSummary{total,completed,percentCompleted},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},id}
				...on PullRequest{number,url,state,createdAt,title,body,author{login,...on User{id,name}},milestone{number,title,description,dueOn},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},reactionGroups{content,users{totalCount}},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},id}
			}
		}
	}
```
variables: `{"number": 1, "owner": "o", "repo": "r"}`

### `IssueProjectItems` — from `gh issue view 1 -R forge.localhost/o/r --comments`  
Profile: GHES-minimal schema
```graphql
query IssueProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){issue(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}
```
variables: `{"endCursor": null, "name": "r", "number": 1, "owner": "o"}`

### `IssueRepositoryInfo` — from `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x`  
Profile: GHES-minimal schema
```graphql
	query IssueRepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			id
			databaseId
			name
			owner { login }
			hasIssuesEnabled
			viewerPermission
		}
	}
```
variables: `{"name": "r", "owner": "o"}`

### `RepositoryMilestoneList` — from `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x`  
Profile: GHES-minimal schema
```graphql
query RepositoryMilestoneList($endCursor:String$name:String!$owner:String!$states:[MilestoneState!]!){repository(owner: $owner, name: $name){milestones(states: $states, first: 100, after: $endCursor){nodes{id,title},pageInfo{hasNextPage,endCursor}}}}
```
variables: `{"endCursor": null, "name": "r", "owner": "o", "states": ["OPEN"]}`

### `RepositoryLabelList` — from `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x`  
Profile: GHES-minimal schema
```graphql
query RepositoryLabelList($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){labels(first: 100, orderBy: {field: NAME, direction: ASC}, after: $endCursor){nodes{id,name},pageInfo{hasNextPage,endCursor}}}}
```
variables: `{"endCursor": null, "name": "r", "owner": "o"}`

### `RepositoryAssignableUsers` — from `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x`  
Profile: GHES-minimal schema
```graphql
query RepositoryAssignableUsers($endCursor:String$name:String!$owner:String!){repository(owner: $owner, name: $name){assignableUsers(first: 100, after: $endCursor){nodes{id,login,name},pageInfo{hasNextPage,endCursor}}}}
```
variables: `{"endCursor": null, "name": "r", "owner": "o"}`

### `IssueCreate` — from `gh issue create -R forge.localhost/o/r -t T -b B -l r -a alice -m x`  
Profile: GHES-minimal schema
```graphql
	mutation IssueCreate($input: CreateIssueInput!) {
		createIssue(input: $input) {
			issue {
				id
				url
			}
		}
	}
```
variables: `{"input": {"assigneeIds": ["ID_1"], "body": "B", "labelIds": ["ID_1"], "milestoneId": "ID_1", "projectIds": null, "repositoryId": "ID_1", "title": "T"}}`

### `IssueByNumber` — from `gh issue edit 1 -R forge.localhost/o/r --add-assignee alice`  
Profile: GHES-minimal schema
```graphql
	query IssueByNumber($owner: String!, $repo: String!, $number: Int!) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issue: issueOrPullRequest(number: $number) {
				__typename
				...on Issue{id,number,title,body,url,assignees(first:100){nodes{id,login,name,databaseId},totalCount}}
				...on PullRequest{id,number,title,body,url,assignees(first:100){nodes{id,login,name,databaseId},totalCount}}
			}
		}
	}
```
variables: `{"number": 1, "owner": "o", "repo": "r"}`

### `IssueUpdate` — from `gh issue edit 1 -R forge.localhost/o/r --add-assignee alice`  
Profile: GHES-minimal schema
```graphql
mutation IssueUpdate($input:UpdateIssueInput!){updateIssue(input: $input){__typename}}
```
variables: `{"input": {"id": "ID_1", "assigneeIds": ["ID_1"]}}`

### `UserCurrent` — from `gh issue status -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
query UserCurrent{viewer{login}}
```
variables: `null`

### `IssueStatus` — from `gh issue status -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
fragment issue on Issue{number,title,url,state,updatedAt,labels(first:100){nodes{id,name,description,color},totalCount}}
	query IssueStatus($owner: String!, $repo: String!, $viewer: String!, $per_page: Int = 10) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			assigned: issues(filterBy: {assignee: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
			mentioned: issues(filterBy: {mentioned: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
			authored: issues(filterBy: {createdBy: $viewer, states: OPEN}, first: $per_page, orderBy: {field: UPDATED_AT, direction: DESC}) {
				totalCount
				nodes {
					...issue
				}
			}
		}
	}
```
variables: `{"owner": "o", "repo": "r", "viewer": "alice"}`

### `Release_fields` — from `gh release list -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
query Release_fields{Release: __type(name: "Release"){fields{name}}}
```
variables: `null`

### `RepositoryReleaseList` — from `gh release list -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
query RepositoryReleaseList($direction:OrderDirection!$endCursor:String$name:String!$owner:String!$perPage:Int!){repository(owner: $owner, name: $name){releases(first: $perPage, orderBy: {field: CREATED_AT, direction: $direction}, after: $endCursor){nodes{name,tagName,isDraft,isLatest,isPrerelease,createdAt,publishedAt},pageInfo{hasNextPage,endCursor}}}}
```
variables: `{"direction": "DESC", "endCursor": null, "name": "r", "owner": "o", "perPage": 30}`

### `RepositoryReleaseByTag` — from `gh release view v1 -R forge.localhost/o/r`  
Profile: GHES-minimal schema
```graphql
query RepositoryReleaseByTag($name:String!$owner:String!$tagName:String!){repository(owner: $owner, name: $name){release(tagName: $tagName){databaseId,isDraft}}}
```
variables: `{"name": "r", "owner": "o", "tagName": "v1"}`

### `PullRequestList` — from `gh pr list`  
Profile: GHES-minimal schema
```graphql
fragment pr on PullRequest{number,title,state,url,headRefName,headRepositoryOwner{id,login,...on User{name}},isCrossRepository,isDraft,createdAt}
		query PullRequestList(
			$owner: String!,
			$repo: String!,
			$limit: Int!,
			$endCursor: String,
			$baseBranch: String,
			$headBranch: String,
			$state: [PullRequestState!] = OPEN
		) {
			repository(owner: $owner, name: $repo) {
				pullRequests(
					states: $state,
					baseRefName: $baseBranch,
					headRefName: $headBranch,
					first: $limit,
					after: $endCursor,
					orderBy: {field: CREATED_AT, direction: DESC}
				) {
					totalCount
					nodes {
						...pr
					}
					pageInfo {
						hasNextPage
						endCursor
					}
				}
			}
		}
```
variables: `{"limit": 30, "owner": "o", "repo": "r", "state": ["OPEN"]}`

### `PullRequestSearch` — from `gh pr list --author alice`  
Profile: GHES-minimal schema
```graphql
fragment pr on PullRequest{number,title,state,url,headRefName,headRepositoryOwner{id,login,...on User{name}},isCrossRepository,isDraft,createdAt}
		query PullRequestSearch(
			$q: String!,
			$type: SearchType!,
			$limit: Int!,
			$endCursor: String,
		) {
			search(query: $q, type: $type, first: $limit, after: $endCursor) {
				issueCount
				nodes {
					...pr
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
```
variables: `{"limit": 30, "q": "author:alice repo:o/r state:open type:pr", "type": "ISSUE"}`

### `PullRequestByNumber` — from `gh pr view 1 --comments`  
Profile: GHES-minimal schema
```graphql
	query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
		repository(owner: $owner, name: $repo) {
			pullRequest(number: $pr_number) {url,number,title,state,body,author{login,...on User{id,name}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},isDraft,maintainerCanModify,mergeable,additions,deletions,commits{totalCount},baseRefName,headRefName,headRepositoryOwner{id,login,...on User{name}},headRepository{id,name,nameWithOwner},isCrossRepository,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},reactionGroups{content,users{totalCount}},createdAt,statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}},id}
		}
	}
```
variables: `{"owner": "o", "pr_number": 1, "repo": "r"}`

### `PullRequestProjectItems` — from `gh pr view 1 --comments`  
Profile: GHES-minimal schema
```graphql
query PullRequestProjectItems($endCursor:String$name:String!$number:Int!$owner:String!){repository(owner: $owner, name: $name){pullRequest(number: $number){projectItems(first: 100, after: $endCursor){totalCount,nodes{id,project{id,title},status:fieldValueByName(name: "Status"){... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},pageInfo{hasNextPage,endCursor}}}}}
```
variables: `{"endCursor": null, "name": "r", "number": 1, "owner": "o"}`

### `PullRequest_fields2` — from `gh pr status`  
Profile: GHES-minimal schema
```graphql
query PullRequest_fields2{WorkflowRun: __type(name: "WorkflowRun"){fields(includeDeprecated: true){name}}}
```
variables: `null`

### `PullRequest_fields` — from `gh pr status`  
Profile: GHES-minimal schema
```graphql
query PullRequest_fields{PullRequest: __type(name: "PullRequest"){fields(includeDeprecated: true){name}},StatusCheckRollupContextConnection: __type(name: "StatusCheckRollupContextConnection"){fields(includeDeprecated: true){name}}}
```
variables: `null`

### `PullRequestStatus` — from `gh pr status`  
Profile: GHES-minimal schema
```graphql
	fragment pr on PullRequest {number,title,state,url,isDraft,isCrossRepository,headRefName,headRepositoryOwner{id,login,...on User{name}},mergeStateStatus,baseRef{branchProtectionRule{requiresStrictStatusChecks}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}}}
	fragment prWithReviews on PullRequest {...pr,reviewDecision,latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}}}
	
	query PullRequestStatus($owner: String!, $repo: String!, $headRefName: String!, $viewerQuery: String!, $reviewerQuery: String!, $per_page: Int = 10) {
		repository(owner: $owner, name: $repo) {
			defaultBranchRef {
				name
			}
			pullRequests(headRefName: $headRefName, first: $per_page, orderBy: { field: CREATED_AT, direction: DESC }) {
				totalCount
				edges {
					node {
						...prWithReviews
					}
				}
			}
		}
	
			viewerCreated: search(query: $viewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...prWithReviews
					}
				}
			}
			reviewRequested: search(query: $reviewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...pr
					}
				}
			}
		}
	
```
variables: `{"headRefName": "feature", "number": 0, "owner": "o", "repo": "r", "reviewerQuery": "repo:o/r state:open review-requested:alice", "viewerQuery": "repo:o/r state:open is:pr author:alice"}`

### `RepositoryInfo` — from `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r`  
Profile: GHES-minimal schema
```graphql
	fragment repo on Repository {
		id
		databaseId
		name
		owner { login }
		sshUrl
		hasIssuesEnabled
		description
		hasWikiEnabled
		viewerPermission
		defaultBranchRef {
			name
		}
	}

	query RepositoryInfo($owner: String!, $name: String!) {
		repository(owner: $owner, name: $name) {
			...repo
			parent {
				...repo
			}
			mergeCommitAllowed
			rebaseMergeAllowed
			squashMergeAllowed
		}
	}
```
variables: `{"name": "r", "owner": "o"}`

### `PullRequestForBranch` — from `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r`  
Profile: GHES-minimal schema
```graphql
	query PullRequestForBranch($owner: String!, $repo: String!, $headRefName: String!, $states: [PullRequestState!]) {
		repository(owner: $owner, name: $repo) {
			pullRequests(headRefName: $headRefName, states: $states, first: 30, orderBy: { field: CREATED_AT, direction: DESC }) {
				nodes {url,id,number,state,baseRefName,headRefName,isCrossRepository,headRepositoryOwner{id,login,...on User{name}}}
			}
			defaultBranchRef { name }
		}
	}
```
variables: `{"headRefName": "feature", "owner": "o", "repo": "r", "states": ["OPEN"]}`

### `PullRequestCreate` — from `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r`  
Profile: GHES-minimal schema
```graphql
		mutation PullRequestCreate($input: CreatePullRequestInput!) {
			createPullRequest(input: $input) {
				pullRequest {
					id
					url
				}
			}
	}
```
variables: `{"input": {"baseRefName": "main", "body": "B", "draft": false, "headRefName": "feature", "maintainerCanModify": true, "repositoryId": "ID_1", "title": "T"}}`

### `PullRequestCreateMetadata` — from `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r`  
Profile: GHES-minimal schema
```graphql
		mutation PullRequestCreateMetadata($input: UpdatePullRequestInput!) {
			updatePullRequest(input: $input) { clientMutationId }
		}
```
variables: `{"input": {"assigneeIds": ["ID_1"], "labelIds": ["ID_1"], "pullRequestId": "ID_1"}}`

### `PullRequestCreateRequestReviews` — from `gh pr create -t T -b B --head feature --base main -r alice -a alice -l r`  
Profile: GHES-minimal schema
```graphql
		mutation PullRequestCreateRequestReviews($input: RequestReviewsInput!) {
			requestReviews(input: $input) { clientMutationId }
		}
```
variables: `{"input": {"pullRequestId": "ID_1", "union": true, "userIds": ["ID_1"]}}`

### `PullRequestByNumber` — from `gh pr merge 1 --merge`  
Profile: GHES-minimal schema
```graphql
	query PullRequestByNumber($owner: String!, $repo: String!, $pr_number: Int!) {
		repository(owner: $owner, name: $repo) {
			pullRequest(number: $pr_number) {id,number,state,title,commits(last:1){nodes{commit{oid}}},mergeStateStatus,headRefName,baseRefName,headRefOid,isCrossRepository}
		}
	}
```
variables: `{"owner": "o", "pr_number": 1, "repo": "r"}`

### `PullRequestReviewAdd` — from `gh pr review 1 --comment -b c`  
Profile: GHES-minimal schema
```graphql
mutation PullRequestReviewAdd($input:AddPullRequestReviewInput!){addPullRequestReview(input:$input){clientMutationId}}
```
variables: `{"input": {"pullRequestId": "ID_1", "body": "c", "event": "COMMENT"}}`

### `PullRequestStatusChecks` — from `gh pr checks 1`  
Profile: GHES-minimal schema
```graphql
	query PullRequestStatusChecks($id: ID!, $endCursor: String) {
		node(id: $id) {
			...on PullRequest {
				statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100,after:$endCursor) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description,isRequired(pullRequestId: $id)},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl,isRequired(pullRequestId: $id)}},pageInfo{hasNextPage,endCursor}}}}}}
			}
		}
	}
```
variables: `{"id": "ID_1"}`

### `PullRequestUpdate` — from `gh pr edit 1 --add-reviewer alice`  
Profile: GHES-minimal schema
```graphql
mutation PullRequestUpdate($input:UpdatePullRequestInput!){updatePullRequest(input: $input){__typename}}
```
variables: `{"input": {"pullRequestId": "ID_1"}}`

### `IssueList` — from `gh issue list -R forge.localhost/o/r --json number,title,labels,assignees,author,state,createdAt,url,comments,milestone,body,closed,closedAt,updatedAt,id,reactionGroups,projectItems,stateReason`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
fragment issue on Issue {number,title,labels(first:100){nodes{id,name,description,color},totalCount},assignees(first:100){nodes{id,login,name,databaseId},totalCount},author{login,...on User{id,name}},state,createdAt,url,comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},milestone{number,title,description,dueOn},body,closed,closedAt,updatedAt,id,reactionGroups{content,users{totalCount}},projectItems(first:100){nodes{id, project{id,title}, status:fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue{optionId,name}}},totalCount},stateReason}
	query IssueList($owner: String!, $repo: String!, $limit: Int, $endCursor: String, $states: [IssueState!] = OPEN, $assignee: String, $author: String, $mention: String) {
		repository(owner: $owner, name: $repo) {
			hasIssuesEnabled
			issues(first: $limit, after: $endCursor, orderBy: {field: CREATED_AT, direction: DESC}, states: $states, filterBy: {assignee: $assignee, createdBy: $author, mentioned: $mention}) {
				totalCount
				nodes {
					...issue
				}
				pageInfo {
					hasNextPage
					endCursor
				}
			}
		}
	}
	
```
variables: `{"limit": 30, "owner": "o", "repo": "r", "states": ["OPEN"]}`

### `LabelAdd` — from `gh issue edit 1 -R forge.localhost/o/r --title N --add-label r --remove-label r --add-assignee alice --milestone x`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
mutation LabelAdd($input:AddLabelsToLabelableInput!){addLabelsToLabelable(input: $input){__typename}}
```
variables: `{"input": {"labelableId": "ID_1", "labelIds": ["ID_1"]}}`

### `LabelRemove` — from `gh issue edit 1 -R forge.localhost/o/r --title N --add-label r --remove-label r --add-assignee alice --milestone x`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
mutation LabelRemove($input:RemoveLabelsFromLabelableInput!){removeLabelsFromLabelable(input: $input){__typename}}
```
variables: `{"input": {"labelableId": "ID_1", "labelIds": ["ID_1"]}}`

### `PullRequestList` — from `gh pr list --json number,title,headRefName,state,author,labels,isDraft,url,reviewDecision,statusCheckRollup,mergeable,files,commits,reviews,latestReviews,reviewRequests,assignees,milestone,comments,closingIssuesReferences,additions,deletions,autoMergeRequest,mergeCommit,potentialMergeCommit,baseRefName,body,id,changedFiles`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
fragment pr on PullRequest{number,title,headRefName,state,author{login,...on User{id,name}},labels(first:100){nodes{id,name,description,color},totalCount},isDraft,url,reviewDecision,statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}},mergeable,files(first: 100) {nodes {additions,deletions,path,changeType}},commits(first: 100) {nodes {commit {authors(first:100) {nodes {name,email,user{id,login}}},messageHeadline,messageBody,oid,committedDate,authoredDate}}},reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount},latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}},reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},assignees(first:100){nodes{id,login,name,databaseId},totalCount},milestone{number,title,description,dueOn},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},closingIssuesReferences(first: 100) {nodes {id,number,url,repository {id,name,owner {id,login}}}pageInfo{hasNextPage,endCursor}},additions,deletions,autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},mergeCommit{oid},potentialMergeCommit{oid},baseRefName,body,id,changedFiles}
		query PullRequestList(
			$owner: String!,
			$repo: String!,
			$limit: Int!,
			$endCursor: String,
			$baseBranch: String,
			$headBranch: String,
			$state: [PullRequestState!] = OPEN
		) {
			repository(owner: $owner, name: $repo) {
				pullRequests(
					states: $state,
					baseRefName: $baseBranch,
					headRefName: $headBranch,
					first: $limit,
					after: $endCursor,
					orderBy: {field: CREATED_AT, direction: DESC}
				) {
					totalCount
					nodes {
						...pr
					}
					pageInfo {
						hasNextPage
						endCursor
					}
				}
			}
		}
```
variables: `{"limit": 30, "owner": "o", "repo": "r", "state": ["OPEN"]}`

### `PullRequestForBranch` — from `gh pr view`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
	query PullRequestForBranch($owner: String!, $repo: String!, $headRefName: String!, $states: [PullRequestState!]) {
		repository(owner: $owner, name: $repo) {
			pullRequests(headRefName: $headRefName, states: $states, first: 30, orderBy: { field: CREATED_AT, direction: DESC }) {
				nodes {url,number,title,state,body,author{login,...on User{id,name}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},isDraft,maintainerCanModify,mergeable,additions,deletions,commits{totalCount},baseRefName,headRefName,headRepositoryOwner{id,login,...on User{name}},headRepository{id,name,nameWithOwner},isCrossRepository,reviewRequests(first: 100) {nodes {requestedReviewer {__typename,...on User{login,name},...on Bot{login},...on Team{organization{login}name,slug}}}},reviews(first: 100) {nodes {id,author{login},authorAssociation,submittedAt,body,state,commit{oid},reactionGroups{content,users{totalCount}}}pageInfo{hasNextPage,endCursor}totalCount},assignees(first:100){nodes{id,login,name,databaseId},totalCount},labels(first:100){nodes{id,name,description,color},totalCount},milestone{number,title,description,dueOn},comments(first: 100) {nodes {id,author{login,...on User{id,name}},authorAssociation,body,createdAt,includesCreatedEdit,isMinimized,minimizedReason,reactionGroups{content,users{totalCount}},url,viewerDidAuthor},pageInfo{hasNextPage,endCursor},totalCount},reactionGroups{content,users{totalCount}},createdAt,statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description},...on CheckRun {name,checkSuite{workflowRun{workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl}},pageInfo{hasNextPage,endCursor}}}}}},id}
			}
			defaultBranchRef { name }
		}
	}
```
variables: `{"headRefName": "feature", "owner": "o", "repo": "r", "states": null}`

### `PullRequestStatus` — from `gh pr status`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
	fragment pr on PullRequest {number,title,state,url,isDraft,isCrossRepository,headRefName,headRepositoryOwner{id,login,...on User{name}},mergeStateStatus,baseRef{branchProtectionRule{requiresStrictStatusChecks}},autoMergeRequest {authorEmail,commitBody,commitHeadline,mergeMethod,enabledAt,enabledBy{login,...on User{id,name}}},statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts {checkRunCount,checkRunCountsByState {state,count},statusContextCount,statusContextCountsByState {state,count}}}}}}}
	fragment prWithReviews on PullRequest {...pr,reviewDecision,latestReviews(first: 100) {nodes {author{login},authorAssociation,submittedAt,body,state}}}
	
	query PullRequestStatus($owner: String!, $repo: String!, $headRefName: String!, $viewerQuery: String!, $reviewerQuery: String!, $per_page: Int = 10) {
		repository(owner: $owner, name: $repo) {
			defaultBranchRef {
				name
			}
			pullRequests(headRefName: $headRefName, first: $per_page, orderBy: { field: CREATED_AT, direction: DESC }) {
				totalCount
				edges {
					node {
						...prWithReviews
					}
				}
			}
		}
	
			viewerCreated: search(query: $viewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...prWithReviews
					}
				}
			}
			reviewRequested: search(query: $reviewerQuery, type: ISSUE, first: $per_page) {
				totalCount: issueCount
				edges {
					node {
					...pr
					}
				}
			}
		}
	
```
variables: `{"headRefName": "feature", "number": 0, "owner": "o", "repo": "r", "reviewerQuery": "repo:o/r state:open review-requested:alice", "viewerQuery": "repo:o/r state:open is:pr author:alice"}`

### `CommentCreate` — from `gh pr close 1 -c bye`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
mutation CommentCreate($input:AddCommentInput!){addComment(input: $input){commentEdge{node{url}}}}
```
variables: `{"input": {"subjectId": "ID_1", "body": "bye"}}`

### `PullRequestClose` — from `gh pr close 1 -c bye`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
mutation PullRequestClose($input:ClosePullRequestInput!){closePullRequest(input: $input){pullRequest{id}}}
```
variables: `{"input": {"pullRequestId": "ID_1"}}`

### `PullRequestStatusChecks` — from `gh pr checks 1`  
Profile: GHES, full-superset schema (all introspection answers positive)
```graphql
	query PullRequestStatusChecks($id: ID!, $endCursor: String) {
		node(id: $id) {
			...on PullRequest {
				statusCheckRollup: commits(last: 1) {nodes {commit {statusCheckRollup {contexts(first:100,after:$endCursor) {nodes {__typename...on StatusContext {context,state,targetUrl,createdAt,description,isRequired(pullRequestId: $id)},...on CheckRun {name,checkSuite{workflowRun{event,workflow{name}}},status,conclusion,startedAt,completedAt,detailsUrl,isRequired(pullRequestId: $id)}},pageInfo{hasNextPage,endCursor}}}}}}
			}
		}
	}
```
variables: `{"id": "ID_1"}`

