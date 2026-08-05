import { FormEvent, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { ArrowRight, UserPlus } from "lucide-react";
import { Link, Navigate, useNavigate } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { useInstance } from "@/features/instance/queries";
import { registerAccount } from "@/features/session/api";
import { useSession } from "@/features/session/queries";

export default function RegisterRoute() {
  const instance = useInstance();
  const session = useSession();
  const navigate = useNavigate();
  const [form, setForm] = useState({
    name: "",
    username: "",
    email: "",
    affiliation: "",
    password: "",
    confirm: "",
  });
  const [validation, setValidation] = useState<string | null>(null);
  const registration = useMutation({
    mutationFn: () =>
      registerAccount({
        name: form.name.trim(),
        username: form.username.trim(),
        email: form.email.trim(),
        affiliation: form.affiliation.trim() || undefined,
        password: form.password,
      }),
    onSuccess: () => navigate("/login?registered=1", { replace: true }),
  });

  if (session.data?.authenticated) return <Navigate to="/" replace />;
  if (
    instance.data &&
    !instance.data.setup_required &&
    !instance.data.policies.allow_public_signup
  ) {
    return (
      <div className="mx-auto max-w-xl px-4 py-24 text-center sm:px-6">
        <h1 className="text-2xl font-semibold">
          Public registration is closed
        </h1>
        <p className="mt-3 text-sm text-muted-foreground">
          Ask an administrator for an account, or sign in if you already have
          one.
        </p>
        <Button asChild className="mt-6">
          <Link to="/login">Sign in</Link>
        </Button>
      </div>
    );
  }

  const update = (key: keyof typeof form, value: string) =>
    setForm((current) => ({ ...current, [key]: value }));

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (form.password !== form.confirm) {
      setValidation("Passwords do not match.");
      return;
    }
    setValidation(null);
    registration.mutate();
  };

  return (
    <div className="mx-auto max-w-2xl px-4 py-12 sm:px-6 lg:px-8">
      <Card className="shadow-lg shadow-primary/5">
        <CardHeader>
          <div className="flex size-10 items-center justify-center rounded-xl bg-primary/10 text-primary">
            <UserPlus className="size-4" />
          </div>
          <CardTitle className="pt-4 text-2xl">Create your account</CardTitle>
          <p className="text-sm text-muted-foreground">
            Your username becomes part of the named graph that owns your
            designs.
          </p>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="grid gap-4 sm:grid-cols-2">
            <Field label="Full name" htmlFor="name">
              <Input
                id="name"
                value={form.name}
                onChange={(event) => update("name", event.target.value)}
                required
              />
            </Field>
            <Field label="Username" htmlFor="username">
              <Input
                id="username"
                value={form.username}
                onChange={(event) => update("username", event.target.value)}
                pattern="[A-Za-z0-9]+"
                title="Letters and numbers only"
                required
              />
            </Field>
            <Field label="Email" htmlFor="email">
              <Input
                id="email"
                type="email"
                value={form.email}
                onChange={(event) => update("email", event.target.value)}
                required
              />
            </Field>
            <Field label="Affiliation (optional)" htmlFor="affiliation">
              <Input
                id="affiliation"
                value={form.affiliation}
                onChange={(event) => update("affiliation", event.target.value)}
              />
            </Field>
            <Field label="Password" htmlFor="new-password">
              <Input
                id="new-password"
                type="password"
                autoComplete="new-password"
                value={form.password}
                onChange={(event) => update("password", event.target.value)}
                required
              />
            </Field>
            <Field label="Confirm password" htmlFor="confirm-password">
              <Input
                id="confirm-password"
                type="password"
                autoComplete="new-password"
                value={form.confirm}
                onChange={(event) => update("confirm", event.target.value)}
                required
              />
            </Field>

            {(validation || registration.error) && (
              <div
                role="alert"
                className="rounded-lg border border-destructive/25 bg-destructive/5 px-3 py-2.5 text-sm text-destructive sm:col-span-2"
              >
                {validation || (registration.error as Error).message}
              </div>
            )}
            <div className="flex flex-col-reverse gap-3 pt-2 sm:col-span-2 sm:flex-row sm:items-center sm:justify-between">
              <p className="text-sm text-muted-foreground">
                Already have an account?{" "}
                <Link
                  className="font-medium text-primary hover:underline"
                  to="/login"
                >
                  Sign in
                </Link>
              </p>
              <Button type="submit" disabled={registration.isPending}>
                {registration.isPending
                  ? "Creating account…"
                  : "Create account"}
                {!registration.isPending && <ArrowRight />}
              </Button>
            </div>
          </form>
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
