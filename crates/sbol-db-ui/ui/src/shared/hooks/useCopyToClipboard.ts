import { useCallback, useEffect, useRef, useState } from "react";

export type CopyStatus = "idle" | "copied" | "error";

export function useCopyToClipboard(resetAfter = 1600) {
  const [status, setStatus] = useState<CopyStatus>("idle");
  const resetTimer = useRef<number>();

  useEffect(
    () => () => {
      if (resetTimer.current !== undefined) {
        window.clearTimeout(resetTimer.current);
      }
    },
    []
  );

  const copy = useCallback(
    async (value: string) => {
      if (resetTimer.current !== undefined) {
        window.clearTimeout(resetTimer.current);
      }
      try {
        await navigator.clipboard.writeText(value);
        setStatus("copied");
      } catch {
        setStatus("error");
      }
      resetTimer.current = window.setTimeout(
        () => setStatus("idle"),
        resetAfter
      );
    },
    [resetAfter]
  );

  return {
    copy,
    status,
    copied: status === "copied",
    failed: status === "error",
  };
}
