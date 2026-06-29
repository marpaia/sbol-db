/**
 * Friendly placeholder for a feature the active deployment doesn't
 * support. Routes stay registered so deep links resolve; when a
 * capability is off, the route renders this panel instead of its
 * normal content.
 */

import { Ban } from "lucide-react";

export function BackendUnavailable({ feature }: { feature: string }) {
  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-2xl px-8 py-20">
        <div className="flex flex-col items-center gap-3 rounded-lg border bg-card px-6 py-12 text-center">
          <Ban size={28} className="text-muted-foreground/60" aria-hidden />
          <h1 className="text-lg font-semibold tracking-tight">
            {feature} is not available here
          </h1>
          <p className="max-w-sm text-sm text-muted-foreground">
            This deployment isn't configured to support this feature.
          </p>
        </div>
      </div>
    </div>
  );
}
