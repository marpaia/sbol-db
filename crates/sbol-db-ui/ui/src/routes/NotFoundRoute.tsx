import { Link } from "react-router-dom";

import { Button } from "@/components/ui/button";

export default function NotFoundRoute() {
  return (
    <div className="mx-auto max-w-2xl px-4 py-24 text-center sm:px-6">
      <div className="font-mono text-xs text-primary">404</div>
      <h1 className="mt-3 text-3xl font-semibold tracking-tight">
        This page isn’t part of the registry
      </h1>
      <p className="mt-3 text-sm leading-6 text-muted-foreground">
        The address may be old, incomplete, or intended for a machine API
        client.
      </p>
      <div className="mt-7 flex justify-center gap-3">
        <Button asChild>
          <Link to="/">Go home</Link>
        </Button>
        <Button asChild variant="outline">
          <Link to="/search">Browse designs</Link>
        </Button>
      </div>
    </div>
  );
}
