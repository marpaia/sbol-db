/**
 * Monaco-backed editor pane shared between the SQL and SPARQL routes.
 * The parent owns the buffer (per-dialect Zustand state); this is a
 * thin wrapper that wires up the editor, theme, and "Run" keybinding.
 */

import { useEffect, useRef } from "react";
import Editor, { type OnMount } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";

import {
  SBOL_LAB_THEME_DARK,
  SBOL_LAB_THEME_LIGHT,
  setupMonaco,
} from "@/lib/monaco/setup";
import {
  attachValidator,
  type AttachedValidator,
  type Validator,
} from "@/lib/monaco/validation";
import { useTheme } from "@/lib/theme";

export interface EditorPaneProps {
  language: string;
  value: string;
  onChange: (next: string) => void;
  onRun: () => void;
  /** Optional server-side validator. When provided, the editor runs it
   *  on every (debounced) buffer change and renders Monaco markers
   *  for the returned errors. */
  validate?: Validator;
}

export function EditorPane({
  language,
  value,
  onChange,
  onRun,
  validate,
}: EditorPaneProps) {
  const editorRef = useRef<MonacoNS.editor.IStandaloneCodeEditor | null>(null);
  const monacoRef = useRef<typeof MonacoNS | null>(null);
  const validatorRef = useRef<AttachedValidator | null>(null);
  const onRunRef = useRef(onRun);
  useEffect(() => {
    onRunRef.current = onRun;
  }, [onRun]);

  const { resolvedTheme } = useTheme();
  const monacoTheme =
    resolvedTheme === "dark" ? SBOL_LAB_THEME_DARK : SBOL_LAB_THEME_LIGHT;

  // Re-apply the theme on Monaco directly when it changes — `theme`
  // prop on the React wrapper triggers a re-render but Monaco itself
  // also exposes a global setter, which is what actually re-skins the
  // already-mounted editor.
  useEffect(() => {
    const monaco = monacoRef.current;
    if (monaco) monaco.editor.setTheme(monacoTheme);
  }, [monacoTheme]);

  useEffect(() => {
    const editor = editorRef.current;
    const monaco = monacoRef.current;
    if (!editor || !monaco || !validate) {
      validatorRef.current?.dispose();
      validatorRef.current = null;
      return;
    }
    validatorRef.current?.dispose();
    validatorRef.current = attachValidator(monaco, editor, validate);
    return () => {
      validatorRef.current?.dispose();
      validatorRef.current = null;
    };
  }, [validate, language]);

  const handleMount: OnMount = (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;
    setupMonaco();
    // Apply the theme explicitly. The `theme` prop on the React
    // wrapper *should* handle this, but the surrounding useEffect
    // skips its first run because `monacoRef.current` is still null
    // when it executes (handleMount fires later). Without this line
    // the editor mounts with Monaco's default `vs` (light) theme
    // even when the rest of the app is in dark mode.
    monaco.editor.setTheme(monacoTheme);
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
      onRunRef.current();
    });
    if (validate) {
      validatorRef.current?.dispose();
      validatorRef.current = attachValidator(monaco, editor, validate);
    }
  };

  return (
    <div className="h-full w-full overflow-hidden">
      <Editor
        height="100%"
        language={language}
        value={value}
        theme={monacoTheme}
        beforeMount={() => setupMonaco()}
        onMount={handleMount}
        onChange={(v) => onChange(v ?? "")}
        options={{
          fontSize: 13,
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          smoothScrolling: true,
          renderLineHighlight: "line",
          padding: { top: 12, bottom: 12 },
          tabSize: 2,
          insertSpaces: true,
          automaticLayout: true,
          wordWrap: "on",
          fixedOverflowWidgets: true,
        }}
      />
    </div>
  );
}
