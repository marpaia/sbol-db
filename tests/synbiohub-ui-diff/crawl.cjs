#!/usr/bin/env node
/**
 * SynBioHub UI differential harness.
 *
 * Drives the real sbh3frontend UI against sbol-db, records every request the UI
 * makes to the backend (method, path, headers, body), then replays each GET
 * against both sbol-db and classic SynBioHub and diffs the status code AND the
 * response body's structural shape. It exists because the UI's failures are
 * usually shape mismatches that return 200 -- a status-only check misses them
 * (e.g. `/admin/registries` returning `{}` where the UI needs `{registries:[]}`).
 *
 * The UI drives which endpoints are exercised, so coverage follows the routes in
 * ROUTES plus whatever the app fetches on each. Mutating POSTs are recorded but
 * never replayed against classic (they would write to it); they are reported
 * with their sbol-db status only, and the count of un-replayed calls is printed
 * so the coverage gap is explicit.
 *
 * Env:
 *   UI       the frontend base URL         (default http://localhost:3333)
 *   SBOLDB   sbol-db backend base URL      (default http://localhost:18903)
 *   CLASSIC  classic SynBioHub base URL    (default http://localhost:17777)
 *   PW_MODULE  path to a playwright module  (default 'playwright')
 *   OUT      JSON report path              (default ./report.json)
 */

const fs = require('fs');
const path = require('path');

const { chromium } = require(process.env.PW_MODULE || 'playwright');

const UI = process.env.UI || 'http://localhost:3333';
const SBOLDB = process.env.SBOLDB || 'http://localhost:18903';
const CLASSIC = process.env.CLASSIC || 'http://localhost:17777';
const OUT = process.env.OUT || path.join(__dirname, 'report.json');

/**
 * The structural signature of a value: keys and value-types, not values.
 *
 * Arrays union the shapes of ALL elements so the signature is independent of
 * element order (two backends may list the same objects in a different order)
 * and reflects optional fields that only some elements carry (e.g. `role` is a
 * string on a ComponentDefinition and absent on an Activity, so the array's
 * element shape carries `role:string|undefined`). An empty array is `array<>`,
 * which [`shapesMatch`] treats as compatible with any populated array.
 */
function shapeOf(v, depth = 0) {
  if (v === null) return 'null';
  if (Array.isArray(v)) {
    if (!v.length) return 'array<>';
    return `array<${unionShape(v, depth + 1)}>`;
  }
  if (typeof v === 'object') {
    if (depth > 3) return 'object';
    return '{' + Object.keys(v).sort().map((k) => `${k}:${shapeOf(v[k], depth + 1)}`).join(',') + '}';
  }
  return typeof v;
}

/** The union of element shapes across an array: per-key sets of value-types. */
function unionShape(items, depth) {
  const objects = items.filter((x) => x && typeof x === 'object' && !Array.isArray(x));
  if (objects.length !== items.length) {
    // Mixed or scalar elements: union their raw shapes.
    return [...new Set(items.map((x) => shapeOf(x, depth)))].sort().join('|');
  }
  const keyTypes = new Map();
  for (const obj of objects) {
    for (const k of Object.keys(obj)) {
      if (!keyTypes.has(k)) keyTypes.set(k, new Set());
      keyTypes.get(k).add(shapeOf(obj[k], depth + 1));
    }
    // Keys absent from this element are optional across the array.
    for (const k of keyTypes.keys()) if (!(k in obj)) keyTypes.get(k).add('undefined');
  }
  const keys = [...keyTypes.keys()].sort();
  return '{' + keys.map((k) => `${k}:${[...keyTypes.get(k)].sort().join('|')}`).join(',') + '}';
}

/**
 * Whether two shape signatures are compatible. Exact match, or one side is an
 * empty array (`array<>`) and the other is any array: an empty result set is a
 * data difference between the two corpora, not a schema divergence.
 */
function shapesMatch(a, b) {
  if (a === b) return true;
  const emptyArrayVsArray = (x, y) => x === 'array<>' && y.startsWith('array<');
  return emptyArrayVsArray(a, b) || emptyArrayVsArray(b, a);
}

/** Classify a raw response body into a comparable shape signature. */
function bodyShape(contentType, text) {
  if ((contentType || '').includes('json')) {
    try {
      return shapeOf(JSON.parse(text));
    } catch {
      return 'invalid-json';
    }
  }
  if (/^\s*-?\d+\s*$/.test(text)) return 'int-text';
  if (/<html|<!doctype/i.test(text)) return 'html';
  if (text.trim() === '') return 'empty';
  return 'text';
}

/** Only the request headers that change a response: content negotiation + auth. */
function replayHeaders(headers) {
  const keep = {};
  for (const [k, v] of Object.entries(headers)) {
    const lk = k.toLowerCase();
    if (lk === 'accept' || lk === 'content-type' || lk === 'x-authorization') keep[k] = v;
  }
  return keep;
}

async function fetchOne(base, method, pathAndQuery, headers, body) {
  try {
    const res = await fetch(base + pathAndQuery, { method, headers, body, redirect: 'manual' });
    const text = await res.text();
    return { status: res.status, shape: bodyShape(res.headers.get('content-type'), text) };
  } catch (e) {
    return { status: 0, shape: `ERROR:${e.message}` };
  }
}

/** The UI routes to exercise. Object routes are filled in from live data. */
async function buildRoutes() {
  const routes = ['/', '/search', '/root-collections', '/submit', '/login', '/admin'];
  // Discover a few real object pages so the viewing path is covered.
  try {
    const res = await fetch(`${SBOLDB}/search/?offset=0&limit=6`, { headers: { Accept: 'application/json' } });
    const objects = await res.json();
    for (const o of objects.slice(0, 4)) {
      const p = String(o.uri).replace(/^https?:\/\/[^/]+/, '');
      if (p) routes.push(p);
    }
  } catch {
    /* backend may be down; the static routes still exercise the app shell */
  }
  return routes;
}

(async () => {
  const routes = await buildRoutes();
  const browser = await chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();

  // key = "METHOD path" -> { method, path, headers, body, seenStatus }
  const captured = new Map();
  page.on('request', (req) => {
    const url = req.url();
    if (!url.startsWith(SBOLDB)) return;
    const pathAndQuery = url.slice(SBOLDB.length) || '/';
    const key = `${req.method()} ${pathAndQuery}`;
    if (!captured.has(key)) {
      captured.set(key, {
        method: req.method(),
        path: pathAndQuery,
        headers: replayHeaders(req.headers()),
        body: req.postData() || undefined,
      });
    }
  });

  const pageErrors = [];
  page.on('pageerror', (e) => pageErrors.push(String(e.message || e)));

  for (const route of routes) {
    await page.goto(UI + route, { waitUntil: 'networkidle', timeout: 30000 }).catch(() => {});
    await page.waitForTimeout(1500);
  }
  await browser.close();

  // Replay each captured request. GETs go to both backends; mutating methods are
  // recorded with their live status only and never replayed against classic.
  const rows = [];
  let unreplayed = 0;
  for (const req of captured.values()) {
    if (req.method !== 'GET') {
      unreplayed += 1;
      rows.push({ ...req, replayed: false });
      continue;
    }
    const [sbol, classic] = await Promise.all([
      fetchOne(SBOLDB, 'GET', req.path, req.headers),
      fetchOne(CLASSIC, 'GET', req.path, req.headers),
    ]);
    const authScoped = [401, 403].includes(sbol.status) || [401, 403].includes(classic.status);
    // Both sides erroring the same way (e.g. 404) is not a schema divergence.
    const bothError = sbol.status >= 400 && classic.status >= 400;
    const divergent =
      !authScoped &&
      !bothError &&
      (sbol.status !== classic.status || !shapesMatch(sbol.shape, classic.shape));
    rows.push({ method: 'GET', path: req.path, sbol, classic, authScoped, divergent, replayed: true });
  }

  rows.sort((a, b) => Number(b.divergent) - Number(a.divergent));
  const divergences = rows.filter((r) => r.divergent);

  fs.writeFileSync(OUT, JSON.stringify({ routes, rows, pageErrors }, null, 2));

  console.log(`\nUI routes crawled: ${routes.length}`);
  console.log(`Backend requests captured: ${captured.size} (${unreplayed} mutating, not replayed against classic)`);
  console.log(`Page errors: ${pageErrors.length}`);
  if (pageErrors.length) pageErrors.forEach((e) => console.log(`  ! ${e}`));

  console.log(`\n=== DIVERGENCES (status or shape differ; auth-scoped excluded): ${divergences.length} ===`);
  for (const r of divergences) {
    console.log(`\n  ${r.path}`);
    console.log(`    sbol-db: ${r.sbol.status}  ${r.sbol.shape}`);
    console.log(`    classic: ${r.classic.status}  ${r.classic.shape}`);
  }
  if (!divergences.length) console.log('  (none)');

  const authScoped = rows.filter((r) => r.replayed && r.authScoped);
  if (authScoped.length) {
    console.log(`\n=== AUTH-SCOPED (401/403 on a side; verify manually): ${authScoped.length} ===`);
    authScoped.forEach((r) => console.log(`  ${r.path}  sbol-db=${r.sbol.status} classic=${r.classic.status}`));
  }

  console.log(`\nFull report: ${OUT}`);
  process.exit(divergences.length ? 1 : 0);
})();
