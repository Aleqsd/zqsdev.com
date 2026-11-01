// Cloudflare Worker for zqsdev.com and its subdomains, in front of the static/ assets.
// The rules live in site-rules.json (also read by the Python tests); this file applies them:
// - redirects: first match wins; a pattern starting with https:// matches host + path, otherwise the path only;
//   "*" captures anything and ":splat" reuses it; an exact pattern also matches with a trailing slash;
//   status 200 rewrites to another file (or proxies an https:// target); "force": false applies only when no file matches.
// - headers: every matching rule applies, later ones override; they match the public path, before any rewrite.

import RULES from "../site-rules.json";

const PRIMARY = "zqsdev.com";
const HSTS = "max-age=31536000";
const DEFAULT_CACHE = "public, max-age=0, must-revalidate";
// Content types kept from the previous host for files the asset layer types differently.
const TYPES = { ".ttf": "font/ttf", ".ico": "image/vnd.microsoft.icon", ".md": "text/markdown; charset=UTF-8", ".ts": "text/plain; charset=UTF-8" };

const escape = (s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
const glob = (pattern, exactSlash) =>
  new RegExp("^" + escape(pattern).replace(/\*/g, "(.*)") + (exactSlash && !pattern.endsWith("*") ? "/?" : "") + "$");

const REDIRECTS = RULES.redirects.map((r) => ({ ...r, re: glob(r.from, true), full: r.from.startsWith("https://") }));
const HEADERS = RULES.headers.map((h) => ({ re: glob(h.for, false), values: h.values }));

function withHeaders(response, path, extra = {}) {
  const out = new Response(response.body, response);
  out.headers.set("Cache-Control", DEFAULT_CACHE);
  for (const { re, values } of HEADERS) if (re.test(path)) for (const [k, v] of Object.entries(values)) out.headers.set(k, v);
  for (const [k, v] of Object.entries(extra)) out.headers.set(k, v);
  const type = out.headers.get("Content-Type") || "";
  if (/^(text\/|application\/(javascript|json|xml))/.test(type) && !/charset/i.test(type)) out.headers.set("Content-Type", type + "; charset=utf-8");
  out.headers.set("Strict-Transport-Security", HSTS);
  return out;
}

const redirect = (location, status = 301) =>
  new Response(`Redirecting to ${location}`, { status, headers: { Location: location, "Content-Type": "text/plain; charset=utf-8" } });

const NOT_FOUND = `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Page not found</title><body style="font-family:system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1rem"><h1>Page not found</h1><p>This page does not exist. <a href="https://zqsdev.com/">Back to zqsdev.com</a></p></body></html>`;

// Fetches one file from static/ ("/dir/" serves "/dir/index.html"); null when it does not exist.
async function asset(request, env, path) {
  const file = (path.startsWith("/") ? path : "/" + path) + (path.endsWith("/") ? "index.html" : "");
  const url = new URL(request.url);
  url.pathname = file;
  url.search = "";
  const res = await env.ASSETS.fetch(new Request(url, { method: request.method === "HEAD" ? "HEAD" : "GET", headers: request.headers }));
  if (res.status === 404) return null;
  const type = TYPES[file.slice(file.lastIndexOf("."))];
  return [res, type && res.status === 200 ? { "Content-Type": type } : {}];
}

async function proxy(request, target) {
  const headers = new Headers(request.headers);
  headers.delete("host");
  const ip = request.headers.get("cf-connecting-ip");
  if (ip) headers.set("X-Forwarded-For", ip);
  const init = { method: request.method, headers, redirect: "manual" };
  if (!["GET", "HEAD"].includes(request.method)) init.body = request.body;
  let res;
  try {
    res = await fetch(target, init);
  } catch {
    return new Response("Bad gateway", { status: 502, headers: { "Strict-Transport-Security": HSTS } });
  }
  const out = new Response(res.body, res);
  out.headers.set("Strict-Transport-Security", HSTS);
  return out;
}

const notFound = (path) => withHeaders(new Response(NOT_FOUND, { status: 404, headers: { "Content-Type": "text/html; charset=utf-8" } }), path);

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    let host = url.hostname;
    let scheme = url.protocol.replace(":", "");
    // Previews and parity tests: on workers.dev or localhost, act as the host named in x-test-host.
    if (host.endsWith(".workers.dev") || host === "localhost" || host === "127.0.0.1") {
      host = request.headers.get("x-test-host") || PRIMARY;
      scheme = request.headers.get("x-test-scheme") || "https";
    }
    const path = url.pathname;
    const search = url.search;
    if (scheme === "http") return redirect(`https://${host}${path}${search}`);
    if (host === "www." + PRIMARY) {
      const res = redirect(`https://${PRIMARY}${path}${search}`);
      res.headers.set("Strict-Transport-Security", HSTS);
      return res;
    }

    for (const rule of REDIRECTS) {
      const match = rule.re.exec(rule.full ? `https://${host}${path}` : path);
      if (!match) continue;
      if (!rule.force) {
        const existing = await asset(request, env, path);
        if (existing) return withHeaders(existing[0], path, existing[1]);
      }
      const target = rule.to.replace(":splat", match[1] ?? "");
      if (rule.status !== 200) return withHeaders(redirect(target + (target.includes("?") ? "" : search), rule.status), path);
      if (target.startsWith("https://")) return proxy(request, target + search);
      const rewritten = await asset(request, env, target);
      return rewritten ? withHeaders(rewritten[0], path, rewritten[1]) : notFound(path);
    }
    const found = await asset(request, env, path);
    return found ? withHeaders(found[0], path, found[1]) : notFound(path);
  },
};
