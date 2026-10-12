// Loading the pinned OpenAPI description, indexing operations, and converting
// OpenAPI 3.0 schemas to JSON Schema (draft-07) for ajv.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';

export const HERE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const METHODS = ['get', 'put', 'post', 'patch', 'delete', 'head', 'options'];

export function loadLock() {
  const p = path.join(HERE, 'spec.lock.json');
  if (!fs.existsSync(p)) throw new Error('spec.lock.json missing: run `node fetch-spec.mjs --update`');
  return JSON.parse(fs.readFileSync(p, 'utf8'));
}

/** which: 'ghes' | 'dotcom' | path to a json file */
export function loadSpec(which = 'ghes') {
  const lock = loadLock();
  let file;
  if (lock.files[which]) file = path.join(HERE, lock.files[which].local);
  else file = path.resolve(which);
  if (!fs.existsSync(file)) throw new Error(`spec file ${file} missing: run \`node fetch-spec.mjs\``);
  const doc = JSON.parse(fs.readFileSync(file, 'utf8'));
  return { doc, file, lock, name: lock.files[which] ? which : path.basename(file) };
}

function resolveRef(doc, obj) {
  let o = obj, guard = 0;
  while (o && o.$ref && guard++ < 10) {
    if (!o.$ref.startsWith('#/')) throw new Error(`external ref ${o.$ref}`);
    o = o.$ref.slice(2).split('/').reduce((a, k) => a?.[k.replace(/~1/g, '/').replace(/~0/g, '~')], doc);
  }
  return o;
}

/** List of every operation in the description. */
export function indexOperations(doc) {
  const ops = [];
  for (const [p, item] of Object.entries(doc.paths)) {
    for (const m of METHODS) {
      const op = item[m];
      if (!op) continue;
      const params = [...(item.parameters || []), ...(op.parameters || [])].map((x) => resolveRef(doc, x));
      const responses = {};
      for (const [code, r] of Object.entries(op.responses || {})) responses[code] = resolveRef(doc, r);
      ops.push({
        operationId: op.operationId,
        method: m.toUpperCase(),
        path: p,
        tag: op.tags?.[0] || 'untagged',
        summary: op.summary,
        category: op['x-github']?.category,
        subcategory: op['x-github']?.subcategory,
        deprecated: !!op.deprecated,
        params,
        requestBody: op.requestBody ? resolveRef(doc, op.requestBody) : null,
        responses,
        servers: op.servers || null,
      });
    }
  }
  return ops;
}

// ---------------------------------------------------------------------------
// OpenAPI 3.0 Schema Object -> JSON Schema draft-07
// ---------------------------------------------------------------------------
const SPEC_ID = 'gh-spec';
const DROP = new Set(['nullable', 'discriminator', 'example', 'examples', 'xml', 'externalDocs', 'deprecated',
  'readOnly', 'writeOnly', 'description', 'title']);

export function makeConverter({ strictOneOf = false } = {}) {
  const rewriteRef = (ref) => {
    const m = ref.match(/^#\/components\/schemas\/(.+)$/);
    if (m) return `${SPEC_ID}#/definitions/${m[1]}`;
    throw new Error(`unsupported $ref in schema: ${ref}`);
  };
  const conv = (s) => {
    if (s === true || s === false) return s;
    if (!s || typeof s !== 'object' || Array.isArray(s)) return s;
    if (s.$ref) {
      // OAS 3.0: siblings of $ref are ignored. We still honour `nullable` (lenient).
      const r = { $ref: rewriteRef(s.$ref) };
      return s.nullable ? { anyOf: [r, { type: 'null' }] } : r;
    }
    const out = {};
    for (const [k, v] of Object.entries(s)) {
      if (DROP.has(k) || k.startsWith('x-')) continue;
      switch (k) {
        case 'properties':
        case 'patternProperties':
          out[k] = Object.fromEntries(Object.entries(v).map(([pk, pv]) => [pk, conv(pv)]));
          break;
        case 'items':
        case 'not':
        case 'additionalProperties':
          out[k] = Array.isArray(v) ? v.map(conv) : conv(v);
          break;
        case 'allOf':
        case 'anyOf':
          out[k] = v.map(conv);
          break;
        case 'oneOf':
          // GitHub's description uses oneOf for unions whose branches overlap
          // (additionalProperties are allowed), so "exactly one" is routinely
          // violated by real responses. Treat as anyOf unless asked to be strict.
          out[strictOneOf ? 'oneOf' : 'anyOf'] = v.map(conv);
          break;
        case 'exclusiveMinimum':
        case 'exclusiveMaximum':
          if (v === true) out[k] = s[k === 'exclusiveMinimum' ? 'minimum' : 'maximum'];
          else if (typeof v === 'number') out[k] = v;
          break;
        case 'format':
          out.format = v;
          break;
        default:
          out[k] = v; // type, enum, required, min*, max*, pattern, default, const, ...
      }
    }
    if ((out.exclusiveMinimum !== undefined && s.exclusiveMinimum === true)) delete out.minimum;
    if ((out.exclusiveMaximum !== undefined && s.exclusiveMaximum === true)) delete out.maximum;
    if (s.nullable === true) {
      if (typeof out.type === 'string') out.type = [out.type, 'null'];
      else if (Array.isArray(out.type)) { if (!out.type.includes('null')) out.type.push('null'); }
      if (Array.isArray(out.enum) && !out.enum.includes(null)) out.enum = [...out.enum, null];
      if (!out.type) return { anyOf: [out, { type: 'null' }] };
    }
    return out;
  };
  return conv;
}

const EXTRA_FORMATS = ['int32', 'int64', 'float', 'double', 'ssh-key', 'ssh-key fingerprint', 'ip/cidr', 'repo.nwo', 'binary', 'byte', 'password'];

/** Build an ajv instance preloaded with all component schemas. */
export function makeValidatorFactory(doc, { strictOneOf = false } = {}) {
  const conv = makeConverter({ strictOneOf });
  const ajv = new Ajv({ strict: false, allErrors: true, validateFormats: true, discriminator: false, code: { optimize: false } });
  addFormats(ajv);
  for (const f of EXTRA_FORMATS) ajv.addFormat(f, true);
  const definitions = {};
  for (const [name, s] of Object.entries(doc.components?.schemas || {})) definitions[name] = conv(s);
  ajv.addSchema({ $id: SPEC_ID, definitions });
  const cache = new Map();
  return {
    ajv,
    convert: conv,
    /** Compile (cached) a validator for an OpenAPI schema object. */
    compile(key, schema) {
      if (cache.has(key)) return cache.get(key);
      const v = ajv.compile(conv(schema));
      cache.set(key, v);
      return v;
    },
  };
}

/** Pick the documented response object for a status code. */
export function documentedResponse(op, status) {
  const s = String(status);
  return op.responses[s] || op.responses[`${s[0]}XX`] || op.responses.default || null;
}

/** Pick the JSON media type of a response/request body. */
export function jsonMedia(content) {
  if (!content) return null;
  if (content['application/json']) return { type: 'application/json', ...content['application/json'] };
  const k = Object.keys(content).find((c) => /json/.test(c));
  return k ? { type: k, ...content[k] } : null;
}

export function formatErrors(errors, max = 5) {
  return (errors || []).slice(0, max).map((e) => {
    const where = e.instancePath || '(root)';
    let extra = '';
    if (e.keyword === 'required') extra = '';
    else if (e.keyword === 'enum') extra = ` ${JSON.stringify(e.params.allowedValues).slice(0, 120)}`;
    else if (e.keyword === 'additionalProperties') extra = ` (${e.params.additionalProperty})`;
    return `${where} ${e.message}${extra}  [${e.schemaPath.slice(0, 120)}]`;
  });
}
