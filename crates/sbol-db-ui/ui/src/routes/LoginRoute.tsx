import { FormEvent, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, LockKeyhole } from "lucide-react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { createSession } from "@/features/portal/api";
import { portalKeys, useInstance, useSession } from "@/features/portal/queries";
import { deploymentName, PRODUCT_NAME } from "@/lib/product";

export default function LoginRoute() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const instance = useInstance();
  const session = useSession();
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");
  const next = safeNext(params.get("next"));
  const deployment = deploymentName(instance.data?.name);

  const login = useMutation({
    mutationFn: () => createSession(identifier.trim(), password),
    onSuccess: (data) => {
      queryClient.setQueryData(portalKeys.session, data);
      navigate(next, { replace: true });
    },
  });

  if (session.data?.authenticated) return <Navigate to={next} replace />;

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (identifier.trim() && password) login.mutate();
  };

  return (
    <div className="mx-auto grid min-h-[calc(100svh-8rem)] max-w-6xl items-center gap-12 px-4 py-12 sm:px-6 lg:grid-cols-2 lg:px-8">
      <div className="hidden lg:block">
        <div className="flex size-12 items-center justify-center rounded-2xl bg-primary/10 text-primary">
          <LockKeyhole className="size-5" />
        </div>
        <h1 className="mt-6 max-w-lg text-4xl font-semibold tracking-[-0.03em]">
          Your private work stays connected to the public registry.
        </h1>
        <p className="mt-5 max-w-lg text-base leading-7 text-muted-foreground">
          Sign in to see account-scoped designs and shared collections. Admins
          also gain access to the data and operations workspace.
        </p>
      </div>

      <Card className="mx-auto w-full max-w-md shadow-lg shadow-primary/5">
        <CardHeader>
          <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
            {PRODUCT_NAME}
          </p>
          <CardTitle className="pt-2 text-2xl">Sign in</CardTitle>
          <p className="text-sm text-muted-foreground">
            Use your username or email address
            {deployment ? ` for ${deployment}` : ""}.
          </p>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="space-y-4">
            <Field label="Username or email" htmlFor="identifier">
              <Input
                id="identifier"
                name="identifier"
                autoComplete="username"
                autoFocus
                value={identifier}
                onChange={(event) => setIdentifier(event.target.value)}
                required
              />
            </Field>
            <Field label="Password" htmlFor="password">
              <Input
                id="password"
                name="password"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                required
              />
            </Field>
            {login.error && (
              <div
                role="alert"
                className="rounded-lg border border-destructive/25 bg-destructive/5 px-3 py-2.5 text-sm text-destructive"
              >
                {(login.error as Error).message}
              </div>
            )}
            <Button type="submit" className="w-full" disabled={login.isPending}>
              {login.isPending ? "Signing in…" : "Sign in"}
              {!login.isPending && <ArrowRight />}
            </Button>
          </form>

          {instance.data?.policies.allow_public_signup && (
            <p className="mt-6 text-center text-sm text-muted-foreground">
              New to this registry?{" "}
              <Link
                className="font-medium text-primary hover:underline"
                to="/register"
              >
                Create an account
              </Link>
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={htmlFor} className="text-sm font-medium">
        {label}
      </label>
      {children}
    </div>
  );
}

function safeNext(value: string | null): string {
  if (!value?.startsWith("/")) return "/";
  try {
    const target = new URL(value, window.location.origin);
    if (target.origin !== window.location.origin) return "/";
    return `${target.pathname}${target.search}${target.hash}`;
  } catch {
    return "/";
  }
}
