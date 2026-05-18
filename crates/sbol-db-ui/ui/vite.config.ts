import { defineConfig, type PluginOption } from "vite";
import react from "@vitejs/plugin-react";
import svgr from "vite-plugin-svgr";
import http from "node:http";
import path from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";

// Dev-only plugin: Vite's dev server enforces a strict trailing
// slash on `base`. A request to bare `/lab` falls outside the base
// and Vite responds with a confusing "did you mean /lab/?" message.
// Production (Rust server) doesn't care — it serves both. Mirror
// that here with a 301 to `/lab/` so the dev UX matches production.
const redirectBareLab: PluginOption = {
  name: "sbol-lab-redirect-bare-base",
  configureServer(server) {
    server.middlewares.use(
      (req: IncomingMessage, res: ServerResponse, next: () => void) => {
        if (req.url === "/lab" || req.url === "/lab?") {
          res.writeHead(301, { Location: "/lab/" });
          res.end();
          return;
        }
        next();
      }
    );
  },
};

// Backend-API proxy for paths that live at the root of the origin
// (`/ontology`, `/openapi.json`, etc.). These can't go through Vite's
// `server.proxy` config because with `base: "/lab/"` set, Vite's
// base-aware handler intercepts root-level requests before the proxy
// middleware fires and returns 404. Installing this as a
// configureServer middleware puts it ahead of base handling and
// guarantees the forward. Production (Rust server, same origin)
// doesn't need this — there's no proxy hop at all.
const ROOT_API_PREFIXES = [
  "/ontology",
  "/openapi.json",
  "/objects",
  "/documents",
  "/sequences",
  "/jobs",
];
const BACKEND_HOST = "localhost";
const BACKEND_PORT = 8080;

const forwardRootApi: PluginOption = {
  name: "sbol-forward-root-api",
  configureServer(server) {
    server.middlewares.use(
      (req: IncomingMessage, res: ServerResponse, next: () => void) => {
        const url = req.url ?? "";
        const matched = ROOT_API_PREFIXES.some(
          (p) => url === p || url.startsWith(`${p}/`) || url.startsWith(`${p}?`)
        );
        if (!matched) return next();

        const headers = { ...req.headers };
        delete headers["host"];
        const upstream = http.request(
          {
            host: BACKEND_HOST,
            port: BACKEND_PORT,
            method: req.method,
            path: url,
            headers,
          },
          (upstreamRes) => {
            res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
            upstreamRes.pipe(res);
          }
        );
        upstream.on("error", (err) => {
          // eslint-disable-next-line no-console
          console.error(
            `[sbol-forward-root-api] ${req.method} ${url} -> ${BACKEND_HOST}:${BACKEND_PORT}: ${err.message}`
          );
          if (!res.headersSent) {
            res.writeHead(502, { "Content-Type": "text/plain" });
          }
          res.end(`Upstream error: ${err.message}`);
        });
        req.pipe(upstream);
      }
    );
  },
};

// `base: "/lab/"` makes every emitted asset URL absolute under /lab/,
// matching the path the Rust server mounts the SPA at. The dev proxy
// forwards backend traffic to the Rust server on port 8080.
export default defineConfig({
  base: "/lab/",
  plugins: [react(), svgr(), redirectBareLab, forwardRootApi],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  server: {
    port: 5173,
    strictPort: true,
    // /lab/api/* lives inside the SPA's base and goes through Vite's
    // own proxy. Root-level backend paths (`/ontology`,
    // `/openapi.json`) are handled by the forwardRootApi plugin
    // above, because Vite's base-aware handler swallows them before
    // proxy middleware fires.
    proxy: {
      "/lab/api": "http://localhost:8080",
    },
  },
  build: {
    target: "es2022",
    chunkSizeWarningLimit: 1024,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom", "react-router-dom"],
          tanstack: [
            "@tanstack/react-query",
            "@tanstack/react-table",
            "@tanstack/react-virtual",
          ],
          monaco: ["monaco-editor", "@monaco-editor/react"],
        },
      },
    },
  },
  optimizeDeps: {
    // Force prebundling of Monaco's worker modules so first-load is
    // fast in the dev server.
    include: ["monaco-editor/esm/vs/editor/editor.api"],
  },
});
