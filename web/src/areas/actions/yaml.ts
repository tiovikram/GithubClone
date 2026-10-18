// A small YAML subset parser for workflow files: block maps and lists, flow lists/maps of
// scalars, quoted/plain scalars, block scalars (| and >) and comments. Enough to read
// `on.workflow_dispatch.inputs` and `jobs.<id>.needs` / `name` for the UI; anything it cannot
// read degrades to "no inputs" / "no graph".

type Y = string | number | boolean | null | Y[] | { [k: string]: Y };

interface Line {
  indent: number;
  text: string;
}

function stripComment(s: string): string {
  let q: string | null = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === "#" && (i === 0 || /\s/.test(s[i - 1]))) return s.slice(0, i).trimEnd();
  }
  return s.trimEnd();
}

function scalar(raw: string): Y {
  const s = raw.trim();
  if (s === "" || s === "~" || s === "null") return null;
  if (s.startsWith('"') && s.endsWith('"')) return s.slice(1, -1).replace(/\\"/g, '"').replace(/\\n/g, "\n");
  if (s.startsWith("'") && s.endsWith("'")) return s.slice(1, -1).replace(/''/g, "'");
  if (s.startsWith("[") && s.endsWith("]")) return splitFlow(s.slice(1, -1)).map(scalar);
  if (s.startsWith("{") && s.endsWith("}")) {
    const o: { [k: string]: Y } = {};
    for (const part of splitFlow(s.slice(1, -1))) {
      const i = part.indexOf(":");
      if (i > 0) o[String(scalar(part.slice(0, i)))] = scalar(part.slice(i + 1));
    }
    return o;
  }
  if (s === "true") return true;
  if (s === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
  return s;
}

function splitFlow(s: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let q: string | null = null;
  let cur = "";
  for (const c of s) {
    if (q) {
      if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === "[" || c === "{") depth++;
    else if (c === "]" || c === "}") depth--;
    else if (c === "," && depth === 0) {
      out.push(cur);
      cur = "";
      continue;
    }
    cur += c;
  }
  if (cur.trim()) out.push(cur);
  return out.map((x) => x.trim()).filter(Boolean);
}

/** `key: value` split at the first unquoted ": " (or trailing ":"). */
function splitKey(t: string): [string, string] | null {
  let q: string | null = null;
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (q) {
      if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === ":" && (i === t.length - 1 || t[i + 1] === " ")) return [t.slice(0, i).trim(), t.slice(i + 1).trim()];
  }
  return null;
}

export function parseYaml(src: string): Y {
  const raw = src.replace(/\r\n?/g, "\n").split("\n");
  const lines: Line[] = [];
  for (let i = 0; i < raw.length; i++) {
    const l = raw[i];
    const indent = l.length - l.trimStart().length;
    const text = stripComment(l.trim());
    if (!text || text === "---") continue;
    // block scalar: swallow the more-indented lines that follow
    if (/(:|^-)\s*[|>][-+]?\d*$/.test(text)) {
      const block: string[] = [];
      while (i + 1 < raw.length && (raw[i + 1].trim() === "" || raw[i + 1].length - raw[i + 1].trimStart().length > indent)) {
        block.push(raw[++i].trim());
      }
      lines.push({ indent, text: text.replace(/[|>][-+]?\d*$/, JSON.stringify(block.join("\n").trim())) });
      continue;
    }
    lines.push({ indent, text });
  }
  let pos = 0;

  function parseBlock(indent: number): Y {
    if (pos >= lines.length) return null;
    if (lines[pos].text.startsWith("- ") || lines[pos].text === "-") return parseList(lines[pos].indent);
    return parseMap(indent);
  }

  function parseMap(indent: number): Y {
    const o: { [k: string]: Y } = {};
    while (pos < lines.length && lines[pos].indent === indent && !lines[pos].text.startsWith("- ")) {
      const kv = splitKey(lines[pos].text);
      pos++;
      if (!kv) continue;
      const [k, v] = kv;
      const key = String(scalar(k));
      if (v !== "") o[key] = scalar(v);
      else if (pos < lines.length && lines[pos].indent > indent) o[key] = parseBlock(lines[pos].indent);
      else if (pos < lines.length && lines[pos].indent === indent && lines[pos].text.startsWith("- ")) o[key] = parseList(indent);
      else o[key] = null;
    }
    return o;
  }

  function parseList(indent: number): Y {
    const a: Y[] = [];
    while (pos < lines.length && lines[pos].indent === indent && (lines[pos].text.startsWith("- ") || lines[pos].text === "-")) {
      const rest = lines[pos].text.slice(1).trim();
      if (!rest) {
        pos++;
        a.push(pos < lines.length && lines[pos].indent > indent ? parseBlock(lines[pos].indent) : null);
        continue;
      }
      if (splitKey(rest) && !/^["'[{]/.test(rest)) {
        // "- key: value" starts a map whose keys are indented to the key column
        const col = indent + (lines[pos].text.length - rest.length);
        lines[pos] = { indent: col, text: rest };
        a.push(parseMap(col));
      } else {
        pos++;
        a.push(scalar(rest));
      }
    }
    return a;
  }

  try {
    return parseBlock(lines[0]?.indent ?? 0);
  } catch {
    return null;
  }
}

const obj = (v: Y | undefined): { [k: string]: Y } => (v && typeof v === "object" && !Array.isArray(v) ? v : {});

export interface DispatchInput {
  name: string;
  description: string;
  required: boolean;
  type: "string" | "boolean" | "choice" | "number" | "environment" | string;
  default: string | boolean;
  options: string[];
}

export interface WorkflowInfo {
  dispatch: boolean;
  inputs: DispatchInput[];
  /** job id -> { name, needs } */
  jobs: { id: string; name: string; needs: string[] }[];
}

export function workflowInfo(src: string): WorkflowInfo {
  const doc = obj(parseYaml(src));
  const on = doc.on ?? doc.true; // YAML 1.1 reads a bare `on` key as true
  let dispatch = false;
  let inputsY: { [k: string]: Y } = {};
  if (typeof on === "string") dispatch = on === "workflow_dispatch";
  else if (Array.isArray(on)) dispatch = on.includes("workflow_dispatch");
  else if (on && typeof on === "object") {
    dispatch = "workflow_dispatch" in on;
    inputsY = obj(obj(on.workflow_dispatch).inputs);
  }
  const inputs: DispatchInput[] = Object.entries(inputsY).map(([name, v]) => {
    const o = obj(v);
    const type = String(o.type ?? "string");
    const def = o.default;
    return {
      name,
      description: String(o.description ?? ""),
      required: o.required === true,
      type,
      default: type === "boolean" ? def === true || def === "true" : def == null ? "" : String(def),
      options: Array.isArray(o.options) ? o.options.map(String) : [],
    };
  });
  const jobs = Object.entries(obj(doc.jobs)).map(([id, v]) => {
    const o = obj(v);
    const needs = Array.isArray(o.needs) ? o.needs.map(String) : typeof o.needs === "string" ? [o.needs] : [];
    return { id, name: typeof o.name === "string" ? o.name : id, needs };
  });
  return { dispatch, inputs, jobs };
}
