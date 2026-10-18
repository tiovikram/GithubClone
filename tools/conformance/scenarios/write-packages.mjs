// Area "packages" scenarios (docs/BUILDOUT.md): the REST packages API over packages published
// through forge's registries, and the Projects (v2) REST API.
//
// Packages are created the way real clients create them: container images are pushed with the
// OCI distribution API (`/v2/`: blob uploads, monolithic and chunked, then manifests), npm
// packages with an `npm publish`-shaped PUT to `/npm/`. Both registries live on the API host's
// origin. Every package is deleted again at the end (deletes are soft on GitHub and forge).
//
// Projects are created and deleted through GraphQL (`createProjectV2`/`deleteProjectV2`): GitHub
// has no REST endpoint for that. Needs `s.issue` (core scenarios) for issue items and --org for
// the organization variants.
import { createHash } from 'node:crypto';

const sha256 = (buf) => `sha256:${createHash('sha256').update(buf).digest('hex')}`;

function origin(t) {
  return new URL(t.base).origin;
}

/** A raw (unvalidated, unrecorded) request against the API host. */
async function raw(t, method, path, { body, headers = {}, token } = {}) {
  const h = { 'user-agent': 'forge-conformance/0.1', authorization: `Bearer ${token ?? t.token}`, ...headers };
  const res = await fetch(origin(t) + path, { method, headers: h, body, redirect: 'manual' });
  const text = await res.text();
  return { status: res.status, headers: res.headers, text };
}

function must(res, ok, what) {
  if (!ok.includes(res.status)) throw new Error(`${what}: HTTP ${res.status} ${res.text.slice(0, 200)}`);
  return res;
}

/** Uploads a blob: monolithically (POST ?digest=) or in two chunks (POST, PATCH, PATCH, PUT). */
async function pushBlob(t, name, buf, chunked) {
  const digest = sha256(buf);
  if (!chunked) {
    must(await raw(t, 'POST', `/v2/${name}/blobs/uploads/?digest=${encodeURIComponent(digest)}`, { body: buf, headers: { 'content-type': 'application/octet-stream' } }), [201], 'monolithic upload');
    return digest;
  }
  const start = must(await raw(t, 'POST', `/v2/${name}/blobs/uploads/`), [202], 'start upload');
  let loc = start.headers.get('location');
  const half = Math.floor(buf.length / 2);
  for (const [from, to] of [[0, half], [half, buf.length]]) {
    const r = must(await raw(t, 'PATCH', loc, {
      body: buf.subarray(from, to),
      headers: { 'content-type': 'application/octet-stream', 'content-range': `${from}-${to - 1}` },
    }), [202], 'upload chunk');
    loc = r.headers.get('location');
  }
  must(await raw(t, 'PUT', `${loc}${loc.includes('?') ? '&' : '?'}digest=${encodeURIComponent(digest)}`), [201], 'finish upload');
  return digest;
}

/** Pushes an OCI image (config + one layer) tagged `tag`; returns the manifest digest. */
async function pushImage(t, name, tag, { source, seed }) {
  const layer = Buffer.from(`forge conformance layer ${seed} ${'x'.repeat(64)}`);
  const config = Buffer.from(JSON.stringify({
    architecture: 'amd64', os: 'linux',
    config: { Labels: { 'org.opencontainers.image.source': source, 'org.opencontainers.image.description': `conformance image ${seed}`, 'org.opencontainers.image.licenses': 'MIT' } },
    rootfs: { type: 'layers', diff_ids: [sha256(layer)] },
  }));
  const layerDigest = await pushBlob(t, name, layer, true);
  const configDigest = await pushBlob(t, name, config, false);
  const manifest = Buffer.from(JSON.stringify({
    schemaVersion: 2,
    mediaType: 'application/vnd.oci.image.manifest.v1+json',
    config: { mediaType: 'application/vnd.oci.image.config.v1+json', digest: configDigest, size: config.length },
    layers: [{ mediaType: 'application/vnd.oci.image.layer.v1.tar+gzip', digest: layerDigest, size: layer.length }],
  }));
  must(await raw(t, 'PUT', `/v2/${name}/manifests/${tag}`, { body: manifest, headers: { 'content-type': 'application/vnd.oci.image.manifest.v1+json' } }), [201], 'push manifest');
  return sha256(manifest);
}

/** `npm publish` of `@scope/name@version` (tarball bytes need not be a real tarball). */
async function npmPublish(t, scope, name, version) {
  const tgz = Buffer.from(`forge conformance npm tarball ${name}@${version}`);
  const full = `@${scope.toLowerCase()}/${name}`;
  const file = `${name}-${version}.tgz`;
  const doc = {
    _id: full, name: full, description: 'forge conformance npm package',
    'dist-tags': { latest: version },
    versions: { [version]: {
      name: full, version, description: 'forge conformance npm package', license: 'MIT',
      dist: { shasum: createHash('sha1').update(tgz).digest('hex'), integrity: `sha512-${createHash('sha512').update(tgz).digest('base64')}`, tarball: `${origin(t)}/npm/${full}/-/${file}` },
    } },
    _attachments: { [file]: { content_type: 'application/octet-stream', data: tgz.toString('base64'), length: tgz.length } },
  };
  must(await raw(t, 'PUT', `/npm/${full.replace('/', '%2f')}`, { body: JSON.stringify(doc), headers: { 'content-type': 'application/json' } }), [200, 201], 'npm publish');
}

/** The package/version operations for one namespace (`user` = /user, `users`, `org`). */
async function packageOps(t, s, kind, params, pkgName) {
  const sfx = { user: 'authenticated-user', users: 'user', org: 'org' }[kind];
  const ids = {
    get: { user: 'packages/get-package-for-authenticated-user', users: 'packages/get-package-for-user', org: 'packages/get-package-for-organization' }[kind],
    versions: { user: 'packages/get-all-package-versions-for-package-owned-by-authenticated-user', users: 'packages/get-all-package-versions-for-package-owned-by-user', org: 'packages/get-all-package-versions-for-package-owned-by-org' }[kind],
    version: { user: 'packages/get-package-version-for-authenticated-user', users: 'packages/get-package-version-for-user', org: 'packages/get-package-version-for-organization' }[kind],
  };
  const p = { ...params, package_type: 'container', package_name: pkgName };
  await t.call(ids.get, { params: p });
  const vs = await t.call(ids.versions, { params: p, minItems: 2 });
  const versions = vs?.body || [];
  if (versions.length < 2) return;
  const vid = versions[versions.length - 1].id;
  await t.call(ids.version, { params: { ...p, package_version_id: vid } });
  await t.call(`packages/delete-package-version-for-${sfx}`, { params: { ...p, package_version_id: vid }, expect: [204] });
  await t.call(ids.versions, { params: p, query: { state: 'deleted' }, minItems: 1 });
  await t.call(`packages/restore-package-version-for-${sfx}`, { params: { ...p, package_version_id: vid }, expect: [204] });
  await t.call(`packages/delete-package-for-${sfx}`, { params: p, expect: [204] });
  await t.call(`packages/restore-package-for-${sfx}`, { params: p, expect: [204] });
}

async function gql(t, query, variables) {
  const r = await raw(t, 'POST', '/api/graphql', { body: JSON.stringify({ query, variables }), headers: { 'content-type': 'application/json' } });
  const j = JSON.parse(r.text || '{}');
  if (j.errors?.length) throw new Error(`graphql: ${j.errors[0].message}`);
  return j.data;
}

async function createProject(t, ownerId, title) {
  const d = await gql(t, 'mutation($o:ID!,$t:String!){createProjectV2(input:{ownerId:$o,title:$t}){projectV2{id number}}}', { o: ownerId, t: title });
  return d.createProjectV2.projectV2;
}

async function deleteProject(t, id) {
  try { await gql(t, 'mutation($p:ID!){deleteProjectV2(input:{projectId:$p}){projectV2{id}}}', { p: id }); } catch { /* best effort */ }
}

/** Every Projects (v2) REST operation for one owner. */
async function projectOps(t, s, kind, project) {
  const isOrg = kind === 'org';
  const P = isOrg ? { org: s.org, project_number: project.number } : { username: s.login, project_number: project.number };
  const id = (o, u) => (isOrg ? o : u);
  await t.call(id('projects/list-for-org', 'projects/list-for-user'), { params: isOrg ? { org: s.org } : { username: s.login }, minItems: 1 });
  await t.call(id('projects/get-for-org', 'projects/get-for-user'), { params: P });
  const fields = await t.call(id('projects/list-fields-for-org', 'projects/list-fields-for-user'), { params: P, minItems: 1 });
  const text = await t.call(id('projects/add-field-for-org', 'projects/add-field-for-user'), {
    params: P, body: { name: 'Notes', data_type: 'text' }, expect: [201],
  });
  const sel = await t.call(id('projects/add-field-for-org', 'projects/add-field-for-user'), {
    params: P, body: { name: 'Priority', data_type: 'single_select', single_select_options: [{ name: 'High', color: 'RED' }, { name: 'Low', color: 'GREEN' }] }, expect: [201],
  });
  await t.call(id('projects/add-field-for-org', 'projects/add-field-for-user'), {
    params: P, body: { name: 'Sprint', data_type: 'iteration', iteration_configuration: { start_date: '2026-01-05', duration: 14, iterations: [{ title: 'Sprint 1', start_date: '2026-01-05', duration: 14 }] } }, expect: [201],
  });
  if (text?.body?.id) await t.call(id('projects/get-field-for-org', 'projects/get-field-for-user'), { params: { ...P, field_id: text.body.id } });

  const draft = isOrg
    ? await t.call('projects/create-draft-item-for-org', { params: P, body: { title: 'Conformance draft', body: 'drafted' }, expect: [201] })
    : await t.call('projects/create-draft-item-for-authenticated-user', { params: { user_id: s.me.id, project_number: project.number }, body: { title: 'Conformance draft', body: 'drafted' }, expect: [201] });
  let issueItem = null;
  if (s.rp && s.issue) {
    issueItem = await t.call(id('projects/add-item-for-org', 'projects/add-item-for-user'), {
      params: P, body: { type: 'Issue', owner: s.rp.owner, repo: s.rp.repo, number: s.issue }, expect: [201],
    });
  }
  const fieldIds = [text?.body?.id, sel?.body?.id, fields?.body?.[0]?.id].filter(Boolean).join(',');
  await t.call(id('projects/list-items-for-org', 'projects/list-items-for-user'), { params: P, query: { fields: fieldIds }, minItems: 1 });
  const itemId = issueItem?.body?.id || draft?.body?.id;
  if (itemId) {
    await t.call(id('projects/get-org-item', 'projects/get-user-item'), { params: { ...P, item_id: itemId }, query: { fields: fieldIds } });
    const fieldsBody = [];
    if (text?.body?.id) fieldsBody.push({ id: text.body.id, value: 'from conformance' });
    if (sel?.body?.options?.[0]?.id) fieldsBody.push({ id: sel.body.id, value: sel.body.options[0].id });
    await t.call(id('projects/update-item-for-org', 'projects/update-item-for-user'), { params: { ...P, item_id: itemId }, body: { fields: fieldsBody } });
  }
  const viewBody = { name: 'Conformance board', layout: 'board', filter: 'is:open', visible_fields: [text?.body?.id].filter(Boolean) };
  const view = isOrg
    ? await t.call('projects/create-view-for-org', { params: P, body: viewBody, expect: [201] })
    : await t.call('projects/create-view-for-user', { params: { user_id: s.me.id, project_number: project.number }, body: viewBody, expect: [201] });
  if (view?.body?.number) {
    await t.call(id('projects/list-view-items-for-org', 'projects/list-view-items-for-user'), { params: { ...P, view_number: view.body.number }, minItems: 1 });
  }
  if (draft?.body?.id) await t.call(id('projects/delete-item-for-org', 'projects/delete-item-for-user'), { params: { ...P, item_id: draft.body.id }, expect: [204] });
}

export const scenarios = [
  {
    name: 'packages: container images and npm packages (user namespace)',
    requires: ['login', 'runId'],
    run: async (t, s) => {
      const img = `conf-${s.runId}-img`;
      const name = `${s.login.toLowerCase()}/${img}`;
      const source = s.rp ? `${origin(t)}/${s.rp.owner}/${s.rp.repo}` : `${origin(t)}/${s.login}/none`;
      await pushImage(t, name, 'v1', { source, seed: 1 });
      await pushImage(t, name, 'v2', { source, seed: 2 });
      await npmPublish(t, s.login, `conf-${s.runId}-npm`, '1.0.0');
      s.pkgUserImage = img;
      try {
        await t.call('packages/list-packages-for-authenticated-user', { query: { package_type: 'container' }, minItems: 1 });
        await t.call('packages/list-packages-for-authenticated-user', { query: { package_type: 'npm', visibility: 'private' } });
        await t.call('packages/list-packages-for-user', { params: { username: s.login }, query: { package_type: 'container' }, minItems: 1 });
        await t.call('packages/list-packages-for-user', { params: { username: s.login }, query: { package_type: 'npm' }, minItems: 1 });
        await t.call('packages/get-package-for-user', { params: { username: s.login, package_type: 'npm', package_name: `conf-${s.runId}-npm` } });
        await t.call('packages/list-docker-migration-conflicting-packages-for-authenticated-user', {});
        await t.call('packages/list-docker-migration-conflicting-packages-for-user', { params: { username: s.login } });
        await packageOps(t, s, 'user', {}, img);
        await packageOps(t, s, 'users', { username: s.login }, img);
      } finally {
        await raw(t, 'DELETE', `/api/v3/user/packages/container/${img}`);
        await raw(t, 'DELETE', `/api/v3/user/packages/npm/conf-${s.runId}-npm`);
      }
    },
  },
  {
    name: 'packages: organization packages',
    requires: ['login', 'org', 'runId'],
    run: async (t, s) => {
      const img = `conf-${s.runId}-oimg`;
      const name = `${s.org.toLowerCase()}/${img}`;
      await pushImage(t, name, 'v1', { source: `${origin(t)}/${s.org}/none`, seed: 3 });
      await pushImage(t, name, 'v2', { source: `${origin(t)}/${s.org}/none`, seed: 4 });
      try {
        await t.call('packages/list-packages-for-organization', { params: { org: s.org }, query: { package_type: 'container' }, minItems: 1 });
        await t.call('packages/list-docker-migration-conflicting-packages-for-organization', { params: { org: s.org } });
        await packageOps(t, s, 'org', { org: s.org }, img);
      } finally {
        await raw(t, 'DELETE', `/api/v3/orgs/${s.org}/packages/container/${img}`);
      }
    },
  },
  {
    name: 'projects: Projects (v2) REST for the authenticated user',
    requires: ['login', 'me', 'runId'],
    run: async (t, s) => {
      const project = await createProject(t, s.me.node_id, `conf-${s.runId} user project`);
      try {
        await projectOps(t, s, 'user', project);
      } finally {
        await deleteProject(t, project.id);
      }
    },
  },
  {
    name: 'projects: Projects (v2) REST for an organization',
    requires: ['login', 'org', 'runId'],
    run: async (t, s) => {
      const org = await t.probe('orgs/get', { params: { org: s.org } });
      const project = await createProject(t, org.body.node_id, `conf-${s.runId} org project`);
      try {
        await projectOps(t, s, 'org', project);
      } finally {
        await deleteProject(t, project.id);
      }
    },
  },
];
