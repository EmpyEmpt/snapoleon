/**
 * Snapoleon CDN worker: serve shared files from R2 (or a public S3 origin) on your
 * own domain, and pass every other request through to snapoleon-server.
 *
 * What it does
 *   /{code}/raw        -> object  raw/{code}
 *   /{code}.png (etc.) -> object  raw/{code}          (file-style links for Markdown/BBCode)
 *   /{code}/thumb.jpg  -> object  thumb/{code}.jpg
 *   /{code}/og.png     -> object  og/{code}.png
 *   anything else      -> ORIGIN  (viewer page, comments, admin, API, short links /s/{code})
 *
 * The server stores public items under exactly those keys. Password-protected items
 * live under private/ and are never mapped here, so a miss (404/403 from the bucket)
 * falls through to the origin, which enforces passwords, expiry and view counting.
 * Objects carry an `expires-ms` metadata value; the worker refuses to serve expired ones.
 *
 * Bindings / variables (wrangler.toml)
 *   ORIGIN         "https://snapoleon-origin.example.com"   the real server (required)
 *   BUCKET         R2 bucket binding                         use this for Cloudflare R2
 *   S3_PUBLIC_URL  "https://bucket.s3.eu-west-1.amazonaws.com"   or any public bucket URL
 *                  (only when BUCKET is not bound; the bucket must allow public reads)
 *
 * Set the server's `public_url` to this worker's domain so generated links point here. To log
 * real client IPs, set `proxy_header = "cf-connecting-ip"` and list in `trusted_proxies` only the
 * networks the worker reaches the origin from (Cloudflare's published ranges, or 127.0.0.1 behind
 * a cloudflared tunnel); the origin should not accept other traffic.
 *
 * Deleted and expiring files stay visible at the edge only for their short Cache-Control
 * (an hour at most, five minutes for expiring items, never past the expiry). Protected items live
 * under random private/ names, so a public bucket does not expose them.
 *
 *   name = "snapoleon-cdn"
 *   main = "cloudflare-worker.js"
 *   compatibility_date = "2025-01-01"
 *   routes = [{ pattern = "snap.example.com/*", zone_name = "example.com" }]
 *   [vars]
 *   ORIGIN = "https://snapoleon-origin.example.com"
 *   [[r2_buckets]]
 *   binding = "BUCKET"
 *   bucket_name = "snapoleon"
 */

// Short codes use the server's alphabet (no 0, 1, i, l, o) and are 8-10 characters.
const CODE = "[2-9a-hj-km-np-z]{8,10}";
const ROUTE = new RegExp(`^/(${CODE})(?:/(raw|thumb\\.jpg|og\\.png)|\\.(?:png|jpe?g|gif|webp|mp4|webm))?$`);

const PUBLIC = "public, max-age=3600";

/** Map a request path to a bucket key, or null when the origin should handle it. */
export function keyFor(pathname) {
  const m = ROUTE.exec(pathname);
  if (!m) return null;
  const [, code, suffix] = m;
  // `/{code}` alone is the viewer page; only suffixed or file-style paths are objects.
  if (pathname === `/${code}`) return null;
  switch (suffix) {
    case "thumb.jpg":
      return `thumb/${code}.jpg`;
    case "og.png":
      return `og/${code}.png`;
    default:
      return `raw/${code}`;
  }
}

function expired(expiresMs) {
  const n = Number(expiresMs);
  return Number.isFinite(n) && n > 0 && n <= Date.now();
}

/** Cache-Control for an object: the stored one, shortened so it never outlives the expiry. */
function cacheControl(stored, expiresMs) {
  const exp = Number(expiresMs);
  if (!Number.isFinite(exp) || exp <= 0) return stored || PUBLIC;
  const left = Math.max(0, Math.floor((exp - Date.now()) / 1000));
  const stale = Number(/max-age=(\d+)/.exec(stored || "")?.[1] ?? 300);
  return `public, max-age=${Math.min(stale, 300, left)}`;
}

function secure(headers) {
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Content-Security-Policy", "default-src 'none'; sandbox");
  headers.set("Accept-Ranges", "bytes");
  return headers;
}

function goneResponse() {
  return new Response("Gone", { status: 410, headers: { "Cache-Control": "no-store" } });
}

/** Serve from an R2 binding. Returns null on a miss so the origin can answer. */
async function fromR2(request, env, key) {
  const obj = await env.BUCKET.get(key, { range: request.headers, onlyIf: request.headers });
  if (obj === null) return null;
  if (expired(obj.customMetadata?.["expires-ms"])) return goneResponse();

  const headers = secure(new Headers());
  obj.writeHttpMetadata(headers);
  headers.set("ETag", obj.httpEtag);
  headers.set("Cache-Control", cacheControl(headers.get("Cache-Control"), obj.customMetadata?.["expires-ms"]));

  // R2 omits the body when a conditional header matched.
  if (!("body" in obj)) return new Response(null, { status: 304, headers });

  let status = 200;
  if (obj.range && request.headers.has("Range")) {
    const { offset = 0, length, suffix } = obj.range;
    const start = suffix !== undefined ? obj.size - suffix : offset;
    const len = suffix !== undefined ? suffix : (length ?? obj.size - offset);
    headers.set("Content-Range", `bytes ${start}-${start + len - 1}/${obj.size}`);
    headers.set("Content-Length", String(len));
    status = 206;
  } else {
    headers.set("Content-Length", String(obj.size));
  }
  return new Response(request.method === "HEAD" ? null : obj.body, { status, headers });
}

/** Serve from a public S3-compatible URL. Returns null on a miss. */
async function fromS3(request, env, key) {
  const upstream = new Request(`${env.S3_PUBLIC_URL.replace(/\/$/, "")}/${key}`, {
    method: request.method,
    headers: pick(request.headers, ["Range", "If-None-Match", "If-Modified-Since"]),
  });
  // No cf.cacheTtl override: the edge honours the object's own Cache-Control, so deletions propagate quickly.
  const resp = await fetch(upstream);
  if (resp.status === 404 || resp.status === 403) return null;
  if (expired(resp.headers.get("x-amz-meta-expires-ms"))) return goneResponse();

  const headers = secure(new Headers());
  for (const name of ["Content-Type", "Content-Length", "Content-Range", "ETag", "Last-Modified"]) {
    const v = resp.headers.get(name);
    if (v) headers.set(name, v);
  }
  headers.set("Cache-Control", cacheControl(resp.headers.get("Cache-Control"), resp.headers.get("x-amz-meta-expires-ms")));
  return new Response(resp.body, { status: resp.status, headers });
}

function pick(headers, names) {
  const out = new Headers();
  for (const n of names) {
    const v = headers.get(n);
    if (v) out.set(n, v);
  }
  return out;
}

/** Forward to the Snapoleon server, preserving method, headers and body. */
function toOrigin(request, env) {
  const url = new URL(request.url);
  const target = new URL(url.pathname + url.search, env.ORIGIN);
  const forwarded = new Request(target, request);
  forwarded.headers.set("X-Forwarded-Host", url.host);
  forwarded.headers.set("X-Forwarded-Proto", url.protocol.replace(":", ""));
  return fetch(forwarded, { redirect: "manual" });
}

export default {
  async fetch(request, env, ctx) {
    if (!env.ORIGIN) return new Response("ORIGIN is not configured", { status: 500 });
    const key = request.method === "GET" || request.method === "HEAD" ? keyFor(new URL(request.url).pathname) : null;
    if (key === null) return toOrigin(request, env);

    // Whole-object GETs can be cached at the edge; ranges and conditionals go straight through.
    const cacheable = request.method === "GET" && !request.headers.has("Range");
    const cache = caches.default;
    if (cacheable) {
      const hit = await cache.match(request);
      if (hit) return hit;
    }

    let resp = null;
    if (env.BUCKET) resp = await fromR2(request, env, key);
    else if (env.S3_PUBLIC_URL) resp = await fromS3(request, env, key);
    if (resp === null) return toOrigin(request, env);

    if (cacheable && resp.status === 200 && resp.headers.get("Cache-Control")?.includes("max-age")) {
      ctx.waitUntil(cache.put(request, resp.clone()));
    }
    return resp;
  },
};
