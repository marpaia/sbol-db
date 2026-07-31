import { defineConfig, type PluginOption } from "vite";
import react from "@vitejs/plugin-react";
import svgr from "vite-plugin-svgr";
import http from "node:http";
import path from "node:path";
import type { IncomingMessage, ServerResponse } from "node:http";

// Backend-API proxy for the root-mounted portal. Some legacy/native API paths
// overlap client-side page families (`/objects/view/*`, `/setup`, `/register`).
// Mirror the Rust dispatch boundary: explicit browser navigation stays with
// Vite, while API methods and machine requests go to the backend.
const ROOT_API_PREFIXES = [
  "/api",
  "/lab/api",
  "/docs",
  "/healthz",
  "/readyz",
  "/metrics",
  "/synbiohub",
  "/ontology",
  "/openapi.json",
  "/objects",
  "/graphs",
  "/sequences",
  "/jobs",
  "/setup",
  "/register",
];
const PORTAL_PAGE_PREFIXES = ["/objects/view", "/setup", "/register"];
const BACKEND_HOST = "localhost";
const BACKEND_PORT = 8888;

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
        const pathname = url.split("?", 1)[0];
        const browserNavigation =
          (req.method === "GET" || req.method === "HEAD") &&
          (req.headers.accept ?? "").includes("text/html") &&
          PORTAL_PAGE_PREFIXES.some(
            (prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`)
          );
        if (browserNavigation) return next();

        const headers = { ...req.headers };
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

// The production portal owns the root origin. The transitional `/lab/*` URLs
// still serve the same index and immediately redirect client-side to `/admin`.
export default defineConfig({
  base: "/",
  plugins: [react(), svgr(), forwardRootApi],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  server: {
    port: 5173,
    strictPort: true,
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
