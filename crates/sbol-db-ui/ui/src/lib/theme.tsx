/**
 * Theme provider. Persists the user's choice in localStorage as one
 * of `light`, `dark`, or `system`. `system` follows
 * `prefers-color-scheme` and reacts to OS-level changes live.
 *
 * The applied class (`dark` / no class) lives on `<html>` so Tailwind
 * `dark:` variants and the CSS-variable palette in globals.css both
 * work without further plumbing.
 *
 * To avoid a flash of the wrong theme on first paint, we read
 * localStorage synchronously in a small init script (see
 * `applyInitialTheme`) called from `main.tsx` before React renders.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "sbol-lab:theme";

function readStored(): Theme {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // localStorage can throw in some sandboxes — fall through to system
  }
  return "system";
}

function systemTheme(): ResolvedTheme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function apply(resolved: ResolvedTheme) {
  const root = document.documentElement;
  root.classList.toggle("dark", resolved === "dark");
  root.style.colorScheme = resolved;
}

/** Synchronous init meant to run before React mounts. */
export function applyInitialTheme() {
  const stored = readStored();
  const resolved = stored === "system" ? systemTheme() : stored;
  apply(resolved);
}

interface ThemeContextValue {
  theme: Theme;
  resolvedTheme: ResolvedTheme;
  setTheme: (t: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(() => readStored());
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>(() =>
    readStored() === "system" ? systemTheme() : (readStored() as ResolvedTheme)
  );

  useEffect(() => {
    const next = theme === "system" ? systemTheme() : theme;
    setResolvedTheme(next);
    apply(next);
  }, [theme]);

  useEffect(() => {
    if (theme !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const next: ResolvedTheme = mq.matches ? "dark" : "light";
      setResolvedTheme(next);
      apply(next);
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // ignore
    }
    setThemeState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, resolvedTheme, setTheme }),
    [theme, resolvedTheme, setTheme]
  );

  return (
    <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used inside <ThemeProvider />");
  return ctx;
}
