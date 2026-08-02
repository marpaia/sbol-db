import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";

const dist = resolve(process.argv[2] ?? "dist");
const html = readFileSync(resolve(dist, "index.html"), "utf8");

const entryMatch = html.match(
  /<script\b[^>]*\btype=["']module["'][^>]*\bsrc=["']([^"']+)["']/i
);
if (!entryMatch) {
  throw new Error("dist/index.html has no module entry script");
}

const entryPath = resolve(dist, entryMatch[1].replace(/^\//, ""));
const entry = readFileSync(entryPath, "utf8");

const modulePreloads = [
  ...html.matchAll(
    /<link\b[^>]*\brel=["']modulepreload["'][^>]*\bhref=["']([^"']+)["']/gi
  ),
].map((match) => match[1]);
const stylesheets = [
  ...html.matchAll(
    /<link\b[^>]*\brel=["']stylesheet["'][^>]*\bhref=["']([^"']+)["']/gi
  ),
].map((match) => match[1]);
const staticImports = [
  ...entry.matchAll(
    /(?:^|;)\s*import(?!\()(?:(?:[^;]*?)from)?["']([^"']+)["']/g
  ),
].map((match) => match[1]);

const allowedRuntimeChunk = /^(?:\.\/|\/assets\/)(?:react|tanstack)-[^/]+\.js$/;
const invalidPreloads = modulePreloads.filter(
  (href) => !allowedRuntimeChunk.test(href)
);
const invalidImports = staticImports.filter(
  (specifier) => !allowedRuntimeChunk.test(specifier)
);
const invalidStyles = stylesheets.filter(
  (href) => !/^\/assets\/index-[^/]+\.css$/.test(href)
);

if (invalidPreloads.length || invalidImports.length || invalidStyles.length) {
  throw new Error(
    [
      "public entry graph includes a route-only bundle",
      invalidPreloads.length
        ? `unexpected module preloads: ${invalidPreloads.join(", ")}`
        : null,
      invalidImports.length
        ? `unexpected static imports: ${invalidImports.join(", ")}`
        : null,
      invalidStyles.length
        ? `unexpected initial stylesheets: ${invalidStyles.join(", ")}`
        : null,
    ]
      .filter(Boolean)
      .join("\n")
  );
}

console.log(
  `Public entry ${basename(entryPath)} loads only ${[
    ...new Set([
      ...modulePreloads.map((href) => basename(href)),
      ...stylesheets.map((href) => basename(href)),
    ]),
  ].join(", ")}`
);
