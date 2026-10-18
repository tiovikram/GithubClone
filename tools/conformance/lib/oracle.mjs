// The oracle: performs calls for scenarios, validates every response against
// the OpenAPI description, and aggregates a per-operation verdict.
import fs from 'node:fs';
import { documentedResponse, jsonMedia, formatErrors } from './spec.mjs';

export class Blocked extends Error {}

const LINK_RELS = new Set(['next', 'prev', 'first', 'last']);
const RANK = { conforming: 0, nonconforming: 1, not_implemented: 2 };

/** Parse an RFC 8288 Link header the way GitHub emits it. Returns {links, errors}. */
export function parseLinkHeader(value) {
  const links = {}, errors = [];
  for (const part of value.split(/,(?=\s*<)/)) {
    const m = part.trim().match(/^<([^>]*)>\s*((?:;\s*[^;]+)*)$/);
    if (!m) { errors.push(`Link: malformed entry ${JSON.stringify(part.trim()).slice(0, 100)}`); continue; }
    const rel = m[2].match(/;\s*rel="?([^";]+)"?/);
    if (!rel) { errors.push(`Link: entry without rel: ${part.trim().slice(0, 100)}`); continue; }
    for (const r of rel[1].split(/\s+/)) {
      if (!LINK_RELS.has(r)) errors.push(`Link: unexpected rel "${r}"`);
      links[r] = m[1];
    }
  }
  return { links, errors };
}

export class Oracle {
  constructor({ ops, validators, prio, base, token, authScheme = 'token', apiVersion = '2022-11-28',
    maxTier = 3, record = null, replay = null, maxRequests = Infinity, verbose = false, timeoutMs = 30000 }) {
    this.byId = new Map(ops.map((o) => [o.operationId, o]));
    this.ops = ops;
    this.v = validators;
    this.prio = prio;
    this.base = base.replace(/\/+$/, '');
    this.token = token;
    this.authScheme = authScheme;
    this.apiVersion = apiVersion;
    this.maxTier = maxTier;
    this.verbose = verbose;
    this.timeoutMs = timeoutMs;
    this.maxRequests = maxRequests;
    this.requests = 0;
    this.results = new Map(); // opId -> {status, calls:[]}
    this.harnessWarnings = [];
    this.recording = record ? [] : null;
    this.recordFile = record;
    this.replay = null;
    if (replay) {
      this.replay = new Map();
      for (const ex of JSON.parse(fs.readFileSync(replay, 'utf8'))) {
        const k = `${ex.method} ${ex.url}`;
        if (!this.replay.has(k)) this.replay.set(k, []);
        this.replay.get(k).push(ex);
      }
    }
    this.state = {};
  }

  op(id) {
    const o = this.byId.get(id);
    if (!o) throw new Error(`unknown operationId ${id} (not in selected spec)`);
    return o;
  }

  buildUrl(op, params = {}, query = {}, urlOverride) {
    let p = op.path.replace(/\{([^}]+)\}/g, (_, name) => {
      if (params[name] === undefined || params[name] === null) throw new Error(`${op.operationId}: missing path param ${name}`);
      return String(params[name]).split('/').map(encodeURIComponent).join('/');
    });
    let url = urlOverride ? urlOverride.replace(/\{[^}]*\}$/, '') : this.base + p;
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(query || {})) if (v !== undefined && v !== null) qs.set(k, String(v));
    const s = qs.toString();
    if (s) url += (url.includes('?') ? '&' : '?') + s;
    return url;
  }

  async http(method, url, { body, headers = {}, rawBody, contentType, token } = {}) {
    if (this.replay) {
      const k = `${method} ${url}`;
      const ex = this.replay.get(k)?.shift();
      if (!ex) throw new Blocked(`replay: no recorded exchange for ${k}`);
      return { status: ex.status, headers: new Headers(ex.headers), text: ex.text };
    }
    if (++this.requests > this.maxRequests) throw new Blocked(`request budget (--max-requests ${this.maxRequests}) exhausted`);
    const h = {
      accept: 'application/vnd.github+json',
      'x-github-api-version': this.apiVersion,
      'user-agent': 'forge-conformance/0.1',
      ...headers,
    };
    // `token` (per call) acts as another user, e.g. the --collaborator-token user.
    const tok = token ?? this.token;
    if (tok) h.authorization = `${this.authScheme} ${tok}`;
    let payload;
    if (rawBody !== undefined) { payload = rawBody; h['content-type'] = contentType || 'application/octet-stream'; }
    else if (body !== undefined) { payload = JSON.stringify(body); h['content-type'] = 'application/json'; }
    const res = await fetch(url, { method, headers: h, body: payload, redirect: 'manual', signal: AbortSignal.timeout(this.timeoutMs) });
    const text = await res.text();
    if (this.recording) this.recording.push({ method, url, status: res.status, headers: Object.fromEntries(res.headers), text });
    return { status: res.status, headers: res.headers, text };
  }

  /**
   * Call an operation by operationId and validate the response.
   * opts: params, query, body, rawBody, contentType, headers, url (override),
   *       expect: [statuses] (default: documented 2xx), fixture: throw Blocked on unexpected status,
   *       note: free text for the report; token: act as another user (e.g. s.collaboratorToken).
   * Returns {status, headers, body, text} or null if skipped by --tier.
   */
  async call(opId, opts = {}) {
    const op = this.op(opId);
    const tier = this.prio.tierOf(opId);
    if (tier > this.maxTier && !opts.fixture) return null;
    this.checkRequestBody(op, opts);
    const url = this.buildUrl(op, opts.params, opts.query, opts.url);
    let res;
    try {
      res = await this.http(op.method, url, opts);
    } catch (e) {
      if (e instanceof Blocked) throw e;
      this.record(op, { url, status: 0, verdict: 'nonconforming', errors: [`request failed: ${e.message}`] });
      if (opts.fixture) throw new Blocked(`${opId}: ${e.message}`);
      return { status: 0, headers: new Headers(), body: null, text: '' };
    }
    const out = this.validate(op, res, opts);
    this.record(op, { url, method: op.method, status: res.status, verdict: out.verdict, errors: out.errors, note: opts.note });
    if (this.verbose || out.verdict !== 'conforming') {
      const tag = out.verdict === 'conforming' ? 'ok ' : out.verdict === 'not_implemented' ? 'N/I' : 'BAD';
      console.log(`  ${tag} ${op.method.padEnd(6)} ${res.status} ${opId}${out.errors.length ? '\n        ' + out.errors.slice(0, 3).join('\n        ') : ''}`);
    }
    const parsed = out.body;
    const result = { status: res.status, headers: res.headers, body: parsed, text: res.text, ok: out.verdict === 'conforming' };
    if (opts.fixture && !out.expected) {
      throw new Blocked(`fixture step ${opId} returned ${res.status}: ${res.text.slice(0, 200)}`);
    }
    return result;
  }

  /** Unvalidated, unrecorded request (for polling async state). */
  async probe(opId, opts = {}) {
    const op = this.op(opId);
    const res = await this.http(op.method, this.buildUrl(op, opts.params, opts.query), opts);
    return { status: res.status, headers: res.headers, body: tryJson(res.text) ?? null, text: res.text };
  }

  /** Poll an operation until pred(res) is true (unrecorded); returns last response. */
  async waitFor(opId, opts, pred, { tries = 8, delayMs = 750 } = {}) {
    let res;
    for (let i = 0; i < tries; i++) {
      res = await this.probe(opId, opts);
      if (pred(res)) return res;
      if (this.replay) break;
      await new Promise((r) => setTimeout(r, delayMs * (i + 1)));
    }
    return res;
  }

  checkRequestBody(op, opts) {
    if (opts.body === undefined || !op.requestBody) return;
    const media = jsonMedia(op.requestBody.content);
    if (!media?.schema) return;
    try {
      const v = this.v.compile(`req:${op.operationId}`, media.schema);
      if (!v(opts.body)) this.harnessWarnings.push(`${op.operationId}: scenario request body does not match spec: ${formatErrors(v.errors, 3).join('; ')}`);
    } catch (e) { this.harnessWarnings.push(`${op.operationId}: cannot compile request schema: ${e.message}`); }
  }

  validate(op, res, opts) {
    const errors = [];
    // Default expectation: the documented success codes (2xx, or 3xx for redirect-only ops such as archives).
    const codes = Object.keys(op.responses);
    const ok2 = codes.filter((c) => /^2\d\d$/.test(c)).map(Number);
    const ok3 = codes.filter((c) => /^3\d\d$/.test(c) && c !== '304').map(Number);
    const expect = opts.expect || (ok2.length ? ok2 : ok3.length ? ok3 : [200]);
    const expected = expect.includes(res.status);
    let verdict = 'conforming';
    let body = null;

    if (!expected && [404, 405, 501].includes(res.status)) {
      return { verdict: 'not_implemented', errors: [`${res.status} for a route that should exist (expected ${expect.join('/')})`], expected, body: tryJson(res.text) };
    }
    const resp = documentedResponse(op, res.status);
    if (!resp) errors.push(`status ${res.status} not documented (documented: ${Object.keys(op.responses).join(', ')})`);
    if (!expected) errors.push(`unexpected status ${res.status} (scenario expected ${expect.join('/')}): ${res.text.slice(0, 160).replace(/\s+/g, ' ')}`);

    const media = resp && jsonMedia(resp.content);
    if (media?.schema) {
      const ct = res.headers.get('content-type') || '';
      if (!/json/.test(ct)) errors.push(`content-type "${ct}" is not JSON`);
      body = tryJson(res.text);
      if (body === undefined) errors.push(`body is not valid JSON: ${res.text.slice(0, 80)}`);
      else {
        const v = this.v.compile(`res:${op.operationId}:${res.status}`, media.schema);
        if (!v(body)) errors.push(...formatErrors(pruneErrors(v.errors), 6).map((e) => `schema: ${e}`));
      }
    } else {
      body = tryJson(res.text) ?? null;
      if (res.status === 204 && res.text.trim()) errors.push('204 response has a body');
    }

    // Lists the scenario knows are non-empty must not come back empty
    // (otherwise `[]` would trivially validate against any array schema).
    if (opts.minItems && expected && body) {
      const items = Array.isArray(body) ? body : Array.isArray(body?.[opts.itemsKey || 'items']) ? body[opts.itemsKey || 'items'] : null;
      if (items && items.length < opts.minItems) errors.push(`expected at least ${opts.minItems} item(s) (fixtures exist), got ${items.length}`);
    }

    // Link pagination header: whenever present it must be well-formed.
    const link = res.headers.get('link');
    if (link) {
      const { links, errors: le } = parseLinkHeader(link);
      errors.push(...le);
      const baseOrigin = new URL(this.base).origin;
      for (const [rel, u] of Object.entries(links)) {
        let url;
        try { url = new URL(u); } catch { errors.push(`Link rel=${rel}: not an absolute URL: ${u}`); continue; }
        if (!this.replay && url.origin !== baseOrigin) errors.push(`Link rel=${rel}: origin ${url.origin} differs from API base ${baseOrigin}`);
        const page = url.searchParams.get('page');
        if (page !== null && !/^\d+$/.test(page)) errors.push(`Link rel=${rel}: non-numeric page=${page}`);
      }
    }
    if (errors.length) verdict = 'nonconforming';
    return { verdict, errors, expected, body };
  }

  /** Request a list with per_page=1 and require working rel=next/last links. */
  async checkPagination(opId, opts = {}) {
    const op = this.op(opId);
    if (this.prio.tierOf(opId) > this.maxTier) return;
    const first = await this.call(opId, { ...opts, query: { ...(opts.query || {}), per_page: 1 }, note: 'pagination page 1' });
    if (!first || first.status !== 200) return;
    const errs = [];
    const items = Array.isArray(first.body) ? first.body : first.body?.items || first.body?.[opts.itemsKey];
    if (Array.isArray(items) && items.length !== 1) errs.push(`per_page=1 returned ${items.length} items`);
    const link = first.headers.get('link');
    if (!link) errs.push('pagination: no Link header although more than one item exists (per_page=1)');
    else {
      const { links } = parseLinkHeader(link);
      if (!links.next) errs.push('pagination: Link header lacks rel="next" on first page');
      // rel="last"/"first" are not required: GitHub's cursor-paginated lists
      // (e.g. issues/list-for-repo) only emit next/prev.
      if (links.next) {
        const res = await this.http(op.method, links.next);
        const out = this.validate(op, res, { expect: [200] });
        this.record(op, { url: links.next, status: res.status, verdict: out.verdict, errors: out.errors, note: 'pagination: followed rel=next' });
        const body2 = out.body;
        const items2 = Array.isArray(body2) ? body2 : body2?.items;
        if (Array.isArray(items2) && Array.isArray(items) && items2.length && items.length && JSON.stringify(items2[0]) === JSON.stringify(items[0])) {
          errs.push('pagination: rel="next" returned the same item as page 1');
        }
        const l2 = res.headers.get('link');
        const rels2 = l2 ? parseLinkHeader(l2).links : {};
        if (!rels2.prev) errs.push('pagination: page 2 Link header lacks rel="prev"');
      }
    }
    if (errs.length) this.record(op, { url: '(pagination check)', status: first.status, verdict: 'nonconforming', errors: errs, note: 'pagination' });
  }

  record(op, call) {
    let r = this.results.get(op.operationId);
    if (!r) { r = { verdict: 'conforming', calls: [] }; this.results.set(op.operationId, r); }
    r.calls.push(call);
    if (RANK[call.verdict] > RANK[r.verdict]) r.verdict = call.verdict;
  }

  saveRecording() {
    if (this.recording && this.recordFile) fs.writeFileSync(this.recordFile, JSON.stringify(this.recording, null, 1));
  }
}

function tryJson(text) {
  if (!text || !text.trim()) return undefined;
  try { return JSON.parse(text); } catch { return undefined; }
}

// anyOf-with-null and similar unions produce noisy sibling errors; keep the
// most specific ones (deepest instancePath first, drop generic "anyOf" errors).
function pruneErrors(errors) {
  if (!errors) return [];
  const useful = errors.filter((e) => e.keyword !== 'anyOf' && e.keyword !== 'oneOf' && !(e.keyword === 'type' && e.params?.type === 'null'));
  const list = useful.length ? useful : errors;
  // Collapse "must have required property" errors for the same object into one.
  const merged = [], req = new Map(), seen = new Set();
  for (const e of list) {
    if (e.keyword === 'required') {
      const k = `${e.instancePath}|${e.schemaPath}`;
      if (!req.has(k)) { const m = { ...e, missing: [] }; req.set(k, m); merged.push(m); }
      req.get(k).missing.push(e.params.missingProperty);
      continue;
    }
    const k = e.instancePath + e.message;
    if (!seen.has(k)) { seen.add(k); merged.push(e); }
  }
  for (const m of req.values()) m.message = `missing required ${m.missing.length > 1 ? 'properties' : 'property'}: ${m.missing.slice(0, 12).join(', ')}${m.missing.length > 12 ? `, ... (+${m.missing.length - 12})` : ''}`;
  return merged;
}
