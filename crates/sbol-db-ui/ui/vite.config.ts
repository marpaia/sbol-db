import { defineConfig, type PluginOption } from "vite";
import react from "@vitejs/plugin-react";
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

// `base: "/lab/"` makes every emitted asset URL absolute under /lab/,
// matching the path the Rust server mounts the SPA at. The dev proxy
// forwards backend traffic to the Rust server on port 8080.
export default defineConfig({
  base: "/lab/",
  plugins: [react(), redirectBareLab],
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  server: {
    port: 5173,
    strictPort: true,
    // /lab/api/* covers all lab-specific endpoints. /ontology is a
    // first-class REST endpoint shared with the CLI / other API
    // consumers — the ontology loader dialog posts to it directly,
    // so it needs a dev-server passthrough too. In production
    // everything lives on the same origin and this proxy isn't
    // involved.
    proxy: {
      "/lab/api": "http://localhost:8080",
      "/ontology": "http://localhost:8080",
      "/openapi.json": "http://localhost:8080",
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
