function parseKeywords(raw) {
  const fields = new Map();
  let i = 0;
  while (i < raw.length) {
    while (/\s/.test(raw[i] ?? "")) i++;
    if (i >= raw.length) break;
    const start = i;
    while (i < raw.length && raw[i] !== "=" && !/\s/.test(raw[i])) i++;
    if (i === start || raw[i] !== "=") throw new Error("keyword DSN is malformed");
    const key = raw.slice(start, i).toLowerCase();
    i++;
    let value = "";
    if (raw[i] === "'") {
      i++;
      let closed = false;
      while (i < raw.length) {
        if (raw[i] === "\\" && i + 1 < raw.length) { i++; value += raw[i++]; }
        else if (raw[i] === "'") { i++; closed = true; break; }
        else value += raw[i++];
      }
      if (!closed || (i < raw.length && !/\s/.test(raw[i]))) throw new Error("keyword DSN quoting is malformed");
    } else {
      while (i < raw.length && !/\s/.test(raw[i])) {
        if (raw[i] === "\\" && i + 1 < raw.length) i++;
        value += raw[i++];
      }
    }
    if (fields.has(key)) throw new Error("duplicate DSN keys are not allowed");
    fields.set(key, value);
  }
  if (fields.has("service") || fields.has("servicefile")) throw new Error("libpq service-file fallback is not allowed");
  return fields;
}

function parseDsn(raw) {
  if (typeof raw !== "string" || !raw.trim()) throw new Error("DSN is required");
  if (/^postgres(?:ql)?:\/\//i.test(raw)) {
    let url;
    try { url = new URL(raw); } catch { throw new Error("URI DSN is malformed"); }
    // Compare case-insensitively like the Rust guard. libpq itself rejects upper-case option names,
    // so this is defence in depth against a client that folds case.
    const identity = new Set(["host", "hostaddr", "port", "user", "dbname", "service", "servicefile"]);
    for (const key of new URLSearchParams(url.search).keys()) {
      if (identity.has(key.toLowerCase())) throw new Error("connection identity parameters must appear only once in the URI authority/path");
    }
    const host = url.hostname;
    const port = url.port ? Number(url.port) : null;
    const user = decodeURIComponent(url.username || "");
    const dbname = url.pathname.replace(/^\//, "");
    const fields = new Map();
    if (host) fields.set("host", host);
    if (port) fields.set("port", String(port));
    if (user) fields.set("user", user);
    if (dbname) fields.set("dbname", dbname);
    return fields;
  }
  return parseKeywords(raw);
}

function isLoopback(host) {
  return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(host.toLowerCase());
}

export function validateTestDsn(raw, {admin = false} = {}) {
  const fields = parseDsn(raw);
  const host = fields.get("host");
  if (!host) throw new Error("host must be explicit");
  if (host.includes(",") || fields.has("hostaddr")) throw new Error("multiple or indirect hosts are not allowed");
  const portStr = fields.get("port");
  if (!portStr) throw new Error("port must be explicit");
  const port = Number(portStr);
  if (!Number.isInteger(port) || port <= 1024 || port === 5432) {
    throw new Error("port must be a dedicated non-default high port");
  }
  const user = fields.get("user");
  if (!user) throw new Error("user must be explicit");
  const dbname = fields.get("dbname");
  if (!dbname) throw new Error("database name must be explicit");
  if (admin) {
    if (!isLoopback(host)) throw new Error("admin host must be loopback for an isolated worker database");
  } else {
    if (!dbname.startsWith("aip_test_")) throw new Error("database name must start with aip_test_");
    if (isLoopback(host) && port === 5432) throw new Error("shared PostgreSQL default endpoint is forbidden");
  }
  return {
    host,
    port,
    user,
    dbname,
    raw,
  };
}

export function requireTestDatabase({admin = false} = {}) {
  for (const key of Object.keys(process.env).filter((name) => name.startsWith("PG"))) {
    throw new Error(`${key} must be unset for isolated DB tests`);
  }
  const key = admin ? "AIP_TEST_ADMIN_URL" : "AIP_TEST_DATABASE_URL";
  const value = process.env[key];
  if (!value) throw new Error(`${key} is required; refusing PostgreSQL defaults`);
  const target = validateTestDsn(value, {admin});
  return target;
}

export function withDatabase(raw, dbname) {
  if (/^postgres(?:ql)?:\/\//i.test(raw)) {
    const url = new URL(raw);
    url.pathname = `/${encodeURIComponent(dbname)}`;
    return url.toString();
  }
  const fields = parseKeywords(raw);
  fields.set("dbname", dbname);
  return [...fields].map(([key, value]) => `${key}='${value.replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`).join(" ");
}

export function withUser(raw, user) {
  if (/^postgres(?:ql)?:\/\//i.test(raw)) {
    const url = new URL(raw);
    url.username = user;
    return url.toString();
  }
  const fields = parseKeywords(raw);
  fields.set("user", user);
  return [...fields].map(([key, value]) => `${key}='${value.replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`).join(" ");
}
