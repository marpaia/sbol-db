import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Shadcn-style className combiner. Resolves Tailwind class conflicts
 * (e.g. `px-2 px-4` → `px-4`) and accepts conditional fragments.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
