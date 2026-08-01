import { FormEvent, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Building2 } from "lucide-react";
import { Navigate, useNavigate } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { setupInstance } from "@/features/portal/api";
import { portalKeys, useInstance } from "@/features/portal/queries";
import { PRODUCT_NAME } from "@/lib/product";

export default function SetupRoute() {
  const instance = useInstance();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const origin = window.location.origin;
  const [form, setForm] = useState({
    instanceName: PRODUCT_NAME,
    instanceUrl: origin,
    uriPrefix: `${origin}/`,
    frontPageText: "",
    allowPublicSignup: true,
    requireLogin: false,
    userName: "",
    userFullName: "",
    userEmail: "",
    affiliation: "",
    userPassword: "",
    userPasswordConfirm: "",
  });
  const [validation, setValidation] = useState<string | null>(null);
  const setup = useMutation({
    mutationFn: () =>
      setupInstance({
        ...form,
        affiliation: form.affiliation.trim() || undefined,
      }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: portalKeys.instance });
      navigate("/login?next=/admin", { replace: true });
    },
  });

  if (instance.data && !instance.data.setup_required) {
    return <Navigate to="/" replace />;
  }

  const update = <K extends keyof typeof form>(
    key: K,
    value: (typeof form)[K]
  ) => setForm((current) => ({ ...current, [key]: value }));

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (form.userPassword !== form.userPasswordConfirm) {
      setValidation("Administrator passwords do not match.");
      return;
    }
    setValidation(null);
    setup.mutate();
  };

  return (
    <div className="mx-auto max-w-4xl px-4 py-12 sm:px-6 lg:px-8">
      <div className="mb-8 max-w-2xl">
        <div className="flex size-11 items-center justify-center rounded-xl bg-primary/10 text-primary">
          <Building2 className="size-5" />
        </div>
        <h1 className="mt-5 text-3xl font-semibold tracking-tight">
          Set up {PRODUCT_NAME}
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">
          Name this deployment, define its public identifiers, and create its
          first administrator. The SBOL DB product identity and design system
          remain consistent across deployments.
        </p>
      </div>

      <form onSubmit={submit} className="space-y-6">
        <Card>
          <CardHeader>
            <CardTitle>Instance identity</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4 sm:grid-cols-2">
            <Field label="Instance name" htmlFor="instance-name">
              <Input
                id="instance-name"
                value={form.instanceName}
                onChange={(event) => update("instanceName", event.target.value)}
                required
              />
              <p className="text-xs text-muted-foreground">
                Optional deployment context, such as your institution or lab.
              </p>
            </Field>
            <Field label="Public instance URL" htmlFor="instance-url">
              <Input
                id="instance-url"
                type="url"
                value={form.instanceUrl}
                onChange={(event) => update("instanceUrl", event.target.value)}
                required
              />
            </Field>
            <Field label="SBOL URI prefix" htmlFor="uri-prefix">
              <Input
                id="uri-prefix"
                type="url"
                value={form.uriPrefix}
                onChange={(event) => update("uriPrefix", event.target.value)}
                required
              />
            </Field>
            <div className="space-y-1.5 sm:col-span-2">
              <label htmlFor="front-page" className="text-sm font-medium">
                Front-page introduction (optional)
              </label>
              <textarea
                id="front-page"
                value={form.frontPageText}
                onChange={(event) =>
                  update("frontPageText", event.target.value)
                }
                rows={3}
                className="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              />
              <p className="text-xs text-muted-foreground">
                SBOL DB renders this as plain text in the initial release.
              </p>
            </div>
            <Checkbox
              checked={form.allowPublicSignup}
              onChange={(checked) => update("allowPublicSignup", checked)}
              label="Allow public account registration"
            />
            <Checkbox
              checked={form.requireLogin}
              onChange={(checked) => update("requireLogin", checked)}
              label="Require login for public browsing"
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>First administrator</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4 sm:grid-cols-2">
            <Field label="Full name" htmlFor="admin-name">
              <Input
                id="admin-name"
                value={form.userFullName}
                onChange={(event) => update("userFullName", event.target.value)}
                required
              />
            </Field>
            <Field label="Username" htmlFor="admin-username">
              <Input
                id="admin-username"
                value={form.userName}
                onChange={(event) => update("userName", event.target.value)}
                required
              />
            </Field>
            <Field label="Email" htmlFor="admin-email">
              <Input
                id="admin-email"
                type="email"
                value={form.userEmail}
                onChange={(event) => update("userEmail", event.target.value)}
                required
              />
            </Field>
            <Field label="Affiliation (optional)" htmlFor="admin-affiliation">
              <Input
                id="admin-affiliation"
                value={form.affiliation}
                onChange={(event) => update("affiliation", event.target.value)}
              />
            </Field>
            <Field label="Password" htmlFor="admin-password">
              <Input
                id="admin-password"
                type="password"
                autoComplete="new-password"
                value={form.userPassword}
                onChange={(event) => update("userPassword", event.target.value)}
                required
              />
            </Field>
            <Field label="Confirm password" htmlFor="admin-confirm">
              <Input
                id="admin-confirm"
                type="password"
                autoComplete="new-password"
                value={form.userPasswordConfirm}
                onChange={(event) =>
                  update("userPasswordConfirm", event.target.value)
                }
                required
              />
            </Field>
          </CardContent>
        </Card>

        {(validation || setup.error) && (
          <div
            role="alert"
            className="rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive"
          >
            {validation || (setup.error as Error).message}
          </div>
        )}

        <div className="flex justify-end">
          <Button type="submit" size="lg" disabled={setup.isPending}>
            {setup.isPending
              ? "Configuring SBOL DB…"
              : "Create SBOL DB instance"}
            {!setup.isPending && <ArrowRight />}
          </Button>
        </div>
      </form>
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

function Checkbox({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3 rounded-lg border bg-muted/15 p-3 text-sm">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="mt-0.5 size-4 accent-[hsl(var(--primary))]"
      />
      <span>{label}</span>
    </label>
  );
}
