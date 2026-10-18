// Area "checks" scenarios (docs/BUILDOUT.md): the Checks API, PR review comment reactions,
// GPG keys, review dismissal and a workflow-run fixture for the Actions reads. They run after the
// core scenarios in write.mjs (repository `s.rp`, `s.featureSha`, `s.baseSha`, `s.login` ...) and
// clean up what they create (the fixture repository itself is deleted by core 'cleanup').
import { createHash, generateKeyPairSync, sign } from 'node:crypto';

const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const P = (s, extra = {}) => ({ ...s.rp, ...extra });
const FORGE_APP_ID = 1; // forge's built-in checks app (no GitHub Apps in forge)

/**
 * A fresh, valid ASCII-armored OpenPGP (RFC 4880) v4 public key: an Ed25519 (EdDSA, algo 22)
 * primary key, a user id and its self-signature (positive certification, SHA-256), generated
 * offline with node:crypto so no gpg binary is needed (`gpg --import` + `--check-sigs` report
 * "1 good signature"). A new key per run avoids "key already exists" on servers that keep keys
 * of earlier runs.
 */
export function gpgPublicKey(uid = 'Conformance Bot <conformance@example.com>', created = Math.floor(Date.now() / 1000)) {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const pub = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32BE(n >>> 0); return b; };
  const u16 = (n) => { const b = Buffer.alloc(2); b.writeUInt16BE(n); return b; };
  const mpi = (buf) => {
    let i = 0; while (i < buf.length - 1 && buf[i] === 0) i++;
    const v = buf.subarray(i);
    return Buffer.concat([u16((v.length - 1) * 8 + (32 - Math.clz32(v[0]))), v]);
  };
  const packet = (tag, body) => {
    const len = body.length < 192 ? Buffer.from([body.length])
      : Buffer.from([((body.length - 192) >> 8) + 192, (body.length - 192) & 0xff]);
    return Buffer.concat([Buffer.from([0xc0 | tag]), len, body]);
  };
  const ed25519Oid = Buffer.from([0x09, 0x2b, 0x06, 0x01, 0x04, 0x01, 0xda, 0x47, 0x0f, 0x01]);
  const keyBody = Buffer.concat([Buffer.from([4]), u32(created), Buffer.from([22]), ed25519Oid, mpi(Buffer.concat([Buffer.from([0x40]), pub]))]);
  const fingerprint = createHash('sha1').update(Buffer.concat([Buffer.from([0x99]), u16(keyBody.length), keyBody])).digest();
  const uidBody = Buffer.from(uid, 'utf8');
  const sub = (type, data) => Buffer.concat([Buffer.from([data.length + 1, type]), data]);
  const hashed = Buffer.concat([
    sub(2, u32(created)),                                    // signature creation time
    sub(27, Buffer.from([0x03])),                            // key flags: certify + sign
    sub(33, Buffer.concat([Buffer.from([4]), fingerprint])), // issuer fingerprint
  ]);
  const sigData = Buffer.concat([Buffer.from([4, 0x13, 22, 8]), u16(hashed.length), hashed]);
  const digest = createHash('sha256').update(Buffer.concat([
    Buffer.from([0x99]), u16(keyBody.length), keyBody,
    Buffer.from([0xb4]), u32(uidBody.length), uidBody,
    sigData, Buffer.from([4, 0xff]), u32(sigData.length),
  ])).digest();
  const rs = sign(null, digest, privateKey); // EdDSA over the hash value; r || s
  const unhashed = sub(16, fingerprint.subarray(12)); // issuer key id
  const sigBody = Buffer.concat([sigData, u16(unhashed.length), unhashed, digest.subarray(0, 2), mpi(rs.subarray(0, 32)), mpi(rs.subarray(32))]);
  const data = Buffer.concat([packet(6, keyBody), packet(13, uidBody), packet(2, sigBody)]);
  let crc = 0xb704ce;
  for (const byte of data) {
    crc ^= byte << 16;
    for (let i = 0; i < 8; i++) { crc <<= 1; if (crc & 0x1000000) crc ^= 0x1864cfb; }
  }
  const crcB64 = Buffer.from([(crc >> 16) & 0xff, (crc >> 8) & 0xff, crc & 0xff]).toString('base64');
  const lines = data.toString('base64').match(/.{1,64}/g).join('\n');
  return {
    keyId: fingerprint.subarray(12).toString('hex').toUpperCase(),
    armored: `-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n${lines}\n=${crcB64}\n-----END PGP PUBLIC KEY BLOCK-----\n`,
  };
}

export const scenarios = [
  {
    // GitHub only lets GitHub Apps create check runs (a PAT gets 403); forge lets writers create
    // them for its built-in app. Against GitHub this scenario stops at the first (fixture) step.
    name: 'checks: runs, annotations, suites',
    requires: ['rp', 'featureSha'],
    run: async (t, s) => {
      const ann = (line, level, message) => ({ path: 'feature.txt', start_line: line, end_line: line, annotation_level: level, message, title: `${level} on line ${line}` });
      const cr = await t.call('checks/create', {
        params: s.rp, expect: [201], fixture: true,
        body: {
          name: 'conformance/check', head_sha: s.featureSha, status: 'in_progress', external_id: `conf-${s.runId}`,
          details_url: 'https://example.com/ci/check', started_at: '2024-01-01T00:00:00Z',
          output: { title: 'Conformance check', summary: 'Running', annotations: [ann(1, 'warning', 'first annotation')] },
        },
      });
      const id = cr.body.id;
      await t.call('checks/get', { params: P(s, { check_run_id: id }) });
      await t.call('checks/update', {
        params: P(s, { check_run_id: id }),
        body: { status: 'completed', conclusion: 'success', completed_at: '2024-01-01T00:05:00Z',
          output: { title: 'Conformance check', summary: 'All good', text: 'details', annotations: [ann(1, 'notice', 'second annotation')] } },
      });
      // annotations are appended across create + update
      await t.call('checks/list-annotations', { params: P(s, { check_run_id: id }), minItems: 2 });
      await t.call('checks/list-for-ref', { params: P(s, { ref: s.featureSha }), minItems: 1, itemsKey: 'check_runs' });
      await t.call('checks/list-for-ref', { params: P(s, { ref: s.featureSha }), query: { check_name: 'conformance/check', filter: 'all' }, minItems: 1, itemsKey: 'check_runs' });
      await t.call('checks/list-suites-for-ref', { params: P(s, { ref: s.featureSha }), minItems: 1, itemsKey: 'check_suites' });
      const suiteId = cr.body.check_suite?.id;
      if (suiteId) {
        await t.call('checks/get-suite', { params: P(s, { check_suite_id: suiteId }) });
        await t.call('checks/list-for-suite', { params: P(s, { check_suite_id: suiteId }), minItems: 1, itemsKey: 'check_runs' });
        await t.call('checks/rerequest-suite', { params: P(s, { check_suite_id: suiteId }) });
      }
      // the suite for this commit already exists -> 200
      await t.call('checks/create-suite', { params: s.rp, body: { head_sha: s.featureSha }, expect: [200] });
      const cr2 = await t.call('checks/create', { params: s.rp, body: { name: 'conformance/done', head_sha: s.featureSha, conclusion: 'neutral' }, expect: [201] });
      if (cr2?.body?.id) await t.call('checks/rerequest-run', { params: P(s, { check_run_id: cr2.body.id }) });

      // suite preferences: with auto_trigger_checks on, a push creates the app's suite
      await t.call('checks/set-suites-preferences', { params: s.rp, body: { auto_trigger_checks: [{ app_id: FORGE_APP_ID, setting: true }] } });
      const branch = `checks-auto-${s.runId}`;
      await t.call('git/create-ref', { params: s.rp, body: { ref: `refs/heads/${branch}`, sha: s.baseSha }, expect: [201], fixture: true });
      const put = await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path: 'checks-auto.txt' }), expect: [201], fixture: true,
        body: { message: 'push for auto-triggered check suite', content: b64('auto\n'), branch },
      });
      await t.call('checks/list-suites-for-ref', { params: P(s, { ref: put.body.commit.sha }), minItems: 1, itemsKey: 'check_suites' });
      await t.call('checks/set-suites-preferences', { params: s.rp, body: { auto_trigger_checks: [{ app_id: FORGE_APP_ID, setting: false }] } });
      await t.call('git/delete-ref', { params: P(s, { ref: `heads/${branch}` }) });
    },
  },
  {
    name: 'checks: gpg keys',
    requires: ['login'],
    run: async (t, s) => {
      const gpg = gpgPublicKey();
      const g = await t.call('users/create-gpg-key-for-authenticated-user', { body: { name: `conformance-${s.runId}`, armored_public_key: gpg.armored }, expect: [201] });
      if (g?.body?.id) {
        await t.call('users/get-gpg-key-for-authenticated-user', { params: { gpg_key_id: g.body.id } });
        await t.call('users/list-gpg-keys-for-authenticated-user', { minItems: 1 });
        await t.call('users/list-gpg-keys-for-user', { params: { username: s.login }, minItems: 1 });
        await t.call('users/delete-gpg-key-for-authenticated-user', { params: { gpg_key_id: g.body.id } });
      }
    },
  },
  {
    // A second user (--collaborator + --collaborator-token) approves; the owner dismisses.
    // Also a check run on the PR head (the run's pull_requests then lists the PR).
    name: 'checks: review dismissal, PR check runs',
    requires: ['rp', 'baseSha', 'collaborator', 'collaboratorToken'],
    run: async (t, s) => {
      const as = s.collaboratorToken;
      const branch = `checks-review-${s.runId}`;
      await t.call('git/create-ref', { params: s.rp, body: { ref: `refs/heads/${branch}`, sha: s.baseSha }, expect: [201], fixture: true });
      await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path: 'checks-review.txt' }), expect: [201], fixture: true,
        body: { message: 'change to review', content: b64('review me\n'), branch },
      });
      const pr = await t.call('pulls/create', { params: s.rp, body: { title: 'Checks: review dismissal', head: branch, base: s.def }, expect: [201], fixture: true });
      const number = pr.body.number;
      const headSha = pr.body.head.sha;
      await t.call('repos/add-collaborator', { params: P(s, { username: s.collaborator }), body: { permission: 'push' }, expect: [201, 204], fixture: true });
      // GitHub invites (201) and the collaborator accepts; forge adds directly (204).
      const inv = await t.probe('repos/list-invitations-for-authenticated-user', { token: as });
      for (const i of (Array.isArray(inv.body) ? inv.body : []).filter((i) => i.repository?.id === s.repoId)) {
        await t.probe('repos/accept-invitation-for-authenticated-user', { params: { invitation_id: i.id }, token: as });
      }
      const ap = await t.call('pulls/create-review', {
        params: P(s, { pull_number: number }), token: as, fixture: true,
        body: { commit_id: headSha, body: 'Looks good', event: 'APPROVE' },
      });
      await t.call('pulls/dismiss-review', {
        params: P(s, { pull_number: number, review_id: ap.body.id }), body: { message: 'dismissed by conformance', event: 'DISMISS' },
      });
      await t.call('pulls/get-review', { params: P(s, { pull_number: number, review_id: ap.body.id }) });
      const cr = await t.call('checks/create', { params: s.rp, body: { name: 'conformance/pr', head_sha: headSha, status: 'queued' }, expect: [201] });
      if (cr?.body?.id) await t.call('checks/get', { params: P(s, { check_run_id: cr.body.id }) });
      await t.call('pulls/update', { params: P(s, { pull_number: number }), body: { state: 'closed' } });
      await t.call('repos/remove-collaborator', { params: P(s, { username: s.collaborator }) });
      await t.call('git/delete-ref', { params: P(s, { ref: `heads/${branch}` }) });
    },
  },
  {
    // The core 'actions (read)' scenario only reads runs if one exists; create one here: a
    // workflow with a workflow_dispatch trigger, dispatched once.
    name: 'checks: workflow run fixture',
    requires: ['rp'],
    run: async (t, s) => {
      const path = `.github/workflows/conformance-${s.runId}.yml`;
      const wf = 'name: Conformance\non:\n  workflow_dispatch:\njobs:\n  noop:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo conformance\n';
      const file = await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path }), expect: [201], fixture: true,
        body: { message: 'Add conformance workflow', content: b64(wf), branch: s.def },
      });
      await t.call('actions/list-repo-workflows', { params: s.rp, minItems: 1, itemsKey: 'workflows' });
      await t.call('actions/create-workflow-dispatch', {
        params: P(s, { workflow_id: path.split('/').pop() }), body: { ref: s.def }, expect: [204], fixture: true,
      });
      await t.waitFor('actions/list-workflow-runs-for-repo', { params: s.rp }, (x) => x.body?.workflow_runs?.length > 0);
      const runs = await t.call('actions/list-workflow-runs-for-repo', { params: s.rp, minItems: 1, itemsKey: 'workflow_runs', fixture: true });
      const run = runs.body.workflow_runs[0];
      await t.call('actions/get-workflow-run', { params: P(s, { run_id: run.id }) });
      await t.call('actions/list-jobs-for-workflow-run', { params: P(s, { run_id: run.id }) });
      await t.probe('actions/cancel-workflow-run', { params: P(s, { run_id: run.id }) });
      await t.call('repos/delete-file', {
        params: P(s, { path }), body: { message: 'Remove conformance workflow', sha: file.body.content.sha, branch: s.def },
      });
    },
  },
];
