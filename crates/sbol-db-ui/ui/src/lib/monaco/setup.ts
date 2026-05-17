/**
 * Centralized Monaco editor configuration.
 *
 * Imported once at module load via the side-effect `setupMonaco()` call
 * from the editor component. Registers the SPARQL/SQL languages and
 * two themes that mirror the app's neutral light/dark palettes.
 */

import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import { registerSparql } from "./sparql-lang";
import { registerSql } from "./sql-lang";

// CRITICAL: by default @monaco-editor/react fetches Monaco from a CDN
// and uses *that* instance for the Editor component. Our `import *
// as monaco from "monaco-editor"` above gives us a *different*,
// locally-bundled instance. Without this call, every `defineTheme`
// /`setTheme` we do happens on the local instance while the React
// wrapper's editors run on the CDN instance — our custom themes
// silently don't exist and Monaco falls back to the default `vs`
// (white) theme.
//
// `loader.config({ monaco })` points the wrapper at the same Monaco
// we've imported, so theme/language registration lines up.
loader.config({ monaco });

let initialized = false;

export function setupMonaco(): void {
  if (initialized) return;
  initialized = true;

  registerSparql(monaco);
  registerSql(monaco);

  // Dark theme — tuned to sit on top of the neutral zinc background
  // used by the rest of the chrome (hsl 240 6% 7%). Each token group
  // gets its own hue so they're cleanly distinguishable against the
  // foreground color (`e4e4e7`) without being noisy.
  monaco.editor.defineTheme("sbol-lab-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "c084fc", fontStyle: "bold" }, // purple
      { token: "predefined", foreground: "c084fc" },
      { token: "type", foreground: "22d3ee" }, // cyan — SQL types (INT, VARCHAR…)
      { token: "type.identifier", foreground: "22d3ee" },
      { token: "operator", foreground: "94a3b8" },
      { token: "delimiter", foreground: "94a3b8" },
      { token: "delimiter.bracket", foreground: "94a3b8" },
      { token: "identifier", foreground: "e4e4e7" },
      { token: "variable", foreground: "fbbf24" }, // amber — SPARQL ?vars
      { token: "iri", foreground: "60a5fa" }, // blue
      { token: "pname", foreground: "60a5fa" },
      { token: "string", foreground: "86efac" }, // green
      { token: "string.quote", foreground: "86efac" },
      { token: "number", foreground: "fdba74" }, // orange
      { token: "comment", foreground: "71717a", fontStyle: "italic" },
    ],
    colors: {
      "editor.background": "#111113",
      "editor.foreground": "#e4e4e7",
      "editorLineNumber.foreground": "#3f3f46",
      "editorLineNumber.activeForeground": "#a1a1aa",
      "editor.selectionBackground": "#3f3f4660",
      "editor.lineHighlightBackground": "#18181b",
      "editorCursor.foreground": "#e4e4e7",
      "editorIndentGuide.background1": "#27272a",
      "editorIndentGuide.activeBackground1": "#3f3f46",
    },
  });

  // Light theme — clean white background, near-black foreground.
  // Same token assignments, darker variants of each hue so they read
  // against white.
  monaco.editor.defineTheme("sbol-lab-light", {
    base: "vs",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "7c3aed", fontStyle: "bold" }, // violet
      { token: "predefined", foreground: "7c3aed" },
      { token: "type", foreground: "0e7490" }, // cyan-700
      { token: "type.identifier", foreground: "0e7490" },
      { token: "operator", foreground: "64748b" },
      { token: "delimiter", foreground: "64748b" },
      { token: "delimiter.bracket", foreground: "64748b" },
      { token: "identifier", foreground: "18181b" },
      { token: "variable", foreground: "a16207" }, // amber-700
      { token: "iri", foreground: "1d4ed8" }, // blue-700
      { token: "pname", foreground: "1d4ed8" },
      { token: "string", foreground: "15803d" }, // green-700
      { token: "string.quote", foreground: "15803d" },
      { token: "number", foreground: "c2410c" }, // orange-700
      { token: "comment", foreground: "71717a", fontStyle: "italic" },
    ],
    colors: {
      "editor.background": "#ffffff",
      "editor.foreground": "#18181b",
      "editorLineNumber.foreground": "#d4d4d8",
      "editorLineNumber.activeForeground": "#52525b",
      "editor.selectionBackground": "#e4e4e7",
      "editor.lineHighlightBackground": "#fafafa",
      "editorCursor.foreground": "#18181b",
      "editorIndentGuide.background1": "#f4f4f5",
      "editorIndentGuide.activeBackground1": "#e4e4e7",
    },
  });
}

export const SBOL_LAB_THEME_DARK = "sbol-lab-dark";
export const SBOL_LAB_THEME_LIGHT = "sbol-lab-light";

// Register themes + languages as a module side-effect so they exist
// the moment anything imports this file. Without this, there's a
// race: the Editor component can construct a Monaco instance and
// apply its `theme` prop before `beforeMount` fires our setup, so
// the named theme doesn't exist yet and Monaco silently falls back
// to its default `vs` (light) theme. The `initialized` guard makes
// repeat calls free.
setupMonaco();
