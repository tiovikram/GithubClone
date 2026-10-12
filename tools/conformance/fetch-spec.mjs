#!/usr/bin/env node
// Fetch GitHub's published OpenAPI descriptions (github/rest-api-description)
// into ./spec/ and pin the upstream commit SHA in ./spec.lock.json.
//
//   node fetch-spec.mjs              # (re)download files pinned in spec.lock.json
//   node fetch-spec.mjs --update     # resolve latest upstream commit + highest GHES, re-pin
//   node fetch-spec.mjs --update --ghes 3.22   # pin a specific GHES version
//   node fetch-spec.mjs --variant deref        # use the fully dereferenced files instead
//
// Downloads go through raw.githubusercontent.com (not subject to the REST API
// rate limit). Only `--update` touches api.github.com (1 request) to list the
// available GHES versions; it falls back to probing raw URLs.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SPEC_DIR = path.join(HERE, 'spec');
const LOCK = path.join(HERE, 'spec.lock.json');
const REPO = 'github/rest-api-description';
const RAW = (sha, p) => `https://raw.githubusercontent.com/${REPO}/${sha}/${p}`;

const args = process.argv.slice(2);
const flag = (n) => args.includes(n);
const opt = (n, d) => { const i = args.indexOf(n); return i >= 0 ? args[i + 1] : d; };

function sha256(buf) { return createHash('sha256').update(buf).digest('hex'); }

async function resolveHeadSha() {
  try {
    const out = execFileSync('git', ['ls-remote', `https://github.com/${REPO}`, 'HEAD'], { encoding: 'utf8', timeout: 60000 });
    const sha = out.split(/\s+/)[0];
    if (/^[0-9a-f]{40}$/.test(sha)) return sha;
  } catch { /* fall through */ }
  const r = await fetch(`https://api.github.com/repos/${REPO}/commits/main`, { headers: { accept: 'application/vnd.github.sha' } });
  if (!r.ok) throw new Error(`cannot resolve upstream HEAD: ${r.status}`);
  return (await r.text()).trim();
}

const cmpVer = (a, b) => { const [a1, a2] = a.split('.').map(Number); const [b1, b2] = b.split('.').map(Number); return a1 - b1 || a2 - b2; };

async function highestGhes(sha) {
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/contents/descriptions?ref=${sha}`, { headers: { accept: 'application/vnd.github+json' } });
    if (r.ok) {
      const vers = (await r.json()).map((e) => e.name).filter((n) => /^ghes-\d+\.\d+$/.test(n)).map((n) => n.slice(5));
      if (vers.length) return vers.sort(cmpVer).pop();
    }
  } catch { /* fall through */ }
  // Fallback: probe raw URLs upward from a known version.
  let [maj, min] = [3, 17], last = null, misses = 0;
  while (misses < 3) {
    const v = `${maj}.${min}`;
    const r = await fetch(RAW(sha, `descriptions/ghes-${v}/ghes-${v}.json`), { method: 'HEAD' });
    if (r.ok) { last = v; misses = 0; } else misses++;
    min++;
  }
  if (!last) throw new Error('could not determine highest GHES version; pass --ghes X.Y');
  return last;
}

function filesFor(ghes, variant) {
  const d = variant === 'deref';
  return {
    ghes: d ? `descriptions/ghes-${ghes}/dereferenced/ghes-${ghes}.deref.json` : `descriptions/ghes-${ghes}/ghes-${ghes}.json`,
    dotcom: d ? 'descriptions/api.github.com/dereferenced/api.github.com.deref.json' : 'descriptions/api.github.com/api.github.com.json',
  };
}

async function download(sha, upstreamPath, expectSha256) {
  const local = path.join(SPEC_DIR, path.basename(upstreamPath));
  if (fs.existsSync(local) && expectSha256 && sha256(fs.readFileSync(local)) === expectSha256) {
    console.log(`ok      ${path.relative(HERE, local)} (cached, checksum matches)`);
    return { local, hash: expectSha256 };
  }
  process.stdout.write(`fetch   ${upstreamPath} @ ${sha.slice(0, 10)} ... `);
  const r = await fetch(RAW(sha, upstreamPath));
  if (!r.ok) throw new Error(`download failed ${r.status} for ${upstreamPath}`);
  const buf = Buffer.from(await r.arrayBuffer());
  JSON.parse(buf.toString('utf8')); // sanity
  const hash = sha256(buf);
  if (expectSha256 && hash !== expectSha256) throw new Error(`checksum mismatch for ${upstreamPath}`);
  fs.writeFileSync(local, buf);
  console.log(`${(buf.length / 1e6).toFixed(1)} MB`);
  return { local, hash };
}

async function main() {
  fs.mkdirSync(SPEC_DIR, { recursive: true });
  let lock = fs.existsSync(LOCK) ? JSON.parse(fs.readFileSync(LOCK, 'utf8')) : null;
  const update = flag('--update') || !lock;
  if (update) {
    const sha = opt('--sha') || await resolveHeadSha();
    const ghes = opt('--ghes') || await highestGhes(sha);
    const variant = opt('--variant', lock?.variant || 'bundled');
    lock = { repo: REPO, sha, ghesVersion: ghes, variant, pinnedAt: new Date().toISOString(), files: {} };
    for (const [k, p] of Object.entries(filesFor(ghes, variant))) lock.files[k] = { upstream: p, local: `spec/${path.basename(p)}` };
  }
  for (const f of Object.values(lock.files)) {
    const { hash } = await download(lock.sha, f.upstream, update ? null : f.sha256);
    f.sha256 = hash;
  }
  fs.writeFileSync(LOCK, JSON.stringify(lock, null, 2) + '\n');
  console.log(`pinned  ${REPO}@${lock.sha} (GHES ${lock.ghesVersion}, ${lock.variant}) -> spec.lock.json`);
}

main().catch((e) => { console.error(e.message); process.exit(1); });
