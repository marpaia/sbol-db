import { TriangleAlert } from "lucide-react";

export function ErrorBanner({ title, body }: { title: string; body?: string }) {
  return (
    <div className="h-full w-full overflow-auto border-t border-destructive/30 bg-destructive/5 p-4">
      <div className="flex items-start gap-3 text-foreground">
        <TriangleAlert size={18} className="mt-0.5 shrink-0 text-destructive" />
        <div className="min-w-0">
          <div className="font-medium">{title}</div>
          {body && (
            <pre className="mt-2 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
              {body}
            </pre>
          )}
        </div>
      </div>
    </div>
  );
}
