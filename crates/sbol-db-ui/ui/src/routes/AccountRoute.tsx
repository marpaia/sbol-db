import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  BadgeCheck,
  Building2,
  Check,
  KeyRound,
  LockKeyhole,
  Mail,
  Save,
  ShieldCheck,
  UserRound,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";

import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import {
  changeAccountPassword,
  updateAccount,
  type PortalApiError,
} from "@/features/portal/api";
import {
  portalKeys,
  useAccount,
  useInstance,
  useSession,
} from "@/features/portal/queries";

export default function AccountRoute() {
  const session = useSession();
  const location = useLocation();
  if (session.isLoading) return <AccountSkeleton />;
  if (!session.data?.authenticated || !session.data.user) {
    const next = `${location.pathname}${location.search}`;
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant="info"
          icon={LockKeyhole}
          title="Sign in to manage your account"
          description="Profile and password settings are available only to the authenticated account."
          action={
            <Button asChild>
              <Link to={`/login?next=${encodeURIComponent(next)}`}>
                Sign in
              </Link>
            </Button>
          }
        />
      </div>
    );
  }
  return <AccountSettings />;
}

function AccountSettings() {
  const account = useAccount();
  const instance = useInstance();

  return (
    <div className="mx-auto w-full max-w-5xl px-4 py-10 sm:px-6 lg:px-8">
      <header className="border-b pb-8">
        <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.14em] text-primary">
          <UserRound className="size-3.5" aria-hidden="true" /> Account
        </div>
        <h1 className="mt-3 text-3xl font-semibold tracking-tight">
          Profile and security
        </h1>
        <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
          Manage the public attribution attached to your work and protect your
          SBOL DB account. Identity and role changes remain administrator-owned.
        </p>
      </header>

      {account.isLoading ? (
        <AccountSkeleton compact />
      ) : account.error ? (
        <SurfaceState
          variant="error"
          title="Couldn’t load account settings"
          description={account.error.message}
          action={
            <Button variant="outline" onClick={() => account.refetch()}>
              Try again
            </Button>
          }
          className="mt-8"
        />
      ) : account.data ? (
        <div className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1.35fr)_minmax(18rem,0.65fr)]">
          <div className="space-y-6">
            <ProfileCard account={account.data} />
            {instance.data?.capabilities.password_change ? (
              <PasswordCard />
            ) : (
              <SurfaceState
                variant="unsupported"
                title="Password changes are unavailable"
                description="This deployment does not advertise local password management."
              />
            )}
          </div>
          <AccountIdentity account={account.data} />
        </div>
      ) : null}
    </div>
  );
}

function ProfileCard({
  account,
}: {
  account: NonNullable<ReturnType<typeof useAccount>["data"]>;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(account.name);
  const [affiliation, setAffiliation] = useState(account.affiliation || "");
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    setName(account.name);
    setAffiliation(account.affiliation || "");
  }, [account.affiliation, account.name]);

  const update = useMutation({
    mutationFn: () => updateAccount({ name, affiliation }),
    onSuccess: (profile) => {
      queryClient.setQueryData(portalKeys.account, profile);
      queryClient.setQueryData(portalKeys.session, {
        authenticated: true,
        user: profile,
      });
      setSaved(true);
    },
  });
  const dirty =
    name.trim() !== account.name ||
    affiliation.trim() !== (account.affiliation || "");

  return (
    <Card>
      <CardHeader className="border-b bg-muted/15">
        <CardTitle className="flex items-center gap-2 text-base">
          <BadgeCheck className="size-4 text-primary" aria-hidden="true" />
          Attribution profile
        </CardTitle>
      </CardHeader>
      <CardContent className="p-5 sm:p-6">
        <form
          className="space-y-5"
          onSubmit={(event) => {
            event.preventDefault();
            setSaved(false);
            update.mutate();
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="account-name">Display name</Label>
            <Input
              id="account-name"
              value={name}
              onChange={(event) => {
                setName(event.target.value);
                setSaved(false);
              }}
              autoComplete="name"
              required
            />
            <p className="text-xs leading-5 text-muted-foreground">
              Used when SBOL DB records you as a contribution or publication
              creator.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="account-affiliation">Affiliation</Label>
            <Input
              id="account-affiliation"
              value={affiliation}
              onChange={(event) => {
                setAffiliation(event.target.value);
                setSaved(false);
              }}
              autoComplete="organization"
              placeholder="Organization or laboratory"
            />
          </div>
          {update.error && <MutationMessage error={update.error} />}
          {saved && !dirty && (
            <p
              className="flex items-center gap-2 text-sm text-success"
              role="status"
            >
              <Check className="size-4" aria-hidden="true" /> Profile saved
            </p>
          )}
          <div className="flex justify-end">
            <Button
              type="submit"
              disabled={!dirty || !name.trim() || update.isPending}
            >
              <Save /> {update.isPending ? "Saving…" : "Save profile"}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}

function PasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [changed, setChanged] = useState(false);
  const mismatch = confirm.length > 0 && next !== confirm;
  const change = useMutation({
    mutationFn: () =>
      changeAccountPassword({
        current_password: current,
        new_password: next,
      }),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
      setChanged(true);
    },
  });

  return (
    <Card>
      <CardHeader className="border-b bg-muted/15">
        <CardTitle className="flex items-center gap-2 text-base">
          <KeyRound className="size-4 text-primary" aria-hidden="true" />
          Change password
        </CardTitle>
      </CardHeader>
      <CardContent className="p-5 sm:p-6">
        <form
          className="space-y-5"
          onSubmit={(event) => {
            event.preventDefault();
            setChanged(false);
            if (!mismatch) change.mutate();
          }}
        >
          <div className="space-y-2">
            <Label htmlFor="current-password">Current password</Label>
            <Input
              id="current-password"
              type="password"
              value={current}
              onChange={(event) => setCurrent(event.target.value)}
              autoComplete="current-password"
              required
            />
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="new-password">New password</Label>
              <Input
                id="new-password"
                type="password"
                value={next}
                onChange={(event) => setNext(event.target.value)}
                autoComplete="new-password"
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="confirm-password">Confirm password</Label>
              <Input
                id="confirm-password"
                type="password"
                value={confirm}
                onChange={(event) => setConfirm(event.target.value)}
                autoComplete="new-password"
                aria-invalid={mismatch}
                required
              />
            </div>
          </div>
          {mismatch && (
            <p className="text-sm text-destructive" role="alert">
              The new passwords do not match.
            </p>
          )}
          {change.error && <MutationMessage error={change.error} />}
          {changed && (
            <p
              className="flex items-center gap-2 text-sm text-success"
              role="status"
            >
              <Check className="size-4" aria-hidden="true" /> Password changed
            </p>
          )}
          <div className="flex justify-end">
            <Button
              type="submit"
              disabled={
                !current || !next || !confirm || mismatch || change.isPending
              }
            >
              <KeyRound /> {change.isPending ? "Changing…" : "Change password"}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}

function AccountIdentity({
  account,
}: {
  account: NonNullable<ReturnType<typeof useAccount>["data"]>;
}) {
  return (
    <Card className="h-fit lg:sticky lg:top-24">
      <CardHeader className="border-b bg-muted/15">
        <CardTitle className="text-base">Account identity</CardTitle>
      </CardHeader>
      <CardContent className="space-y-5 p-5">
        <IdentityRow
          icon={UserRound}
          label="Username"
          value={account.username}
          mono
        />
        <IdentityRow icon={Mail} label="Email" value={account.email} />
        <IdentityRow
          icon={Building2}
          label="Affiliation"
          value={account.affiliation || "Not set"}
        />
        <IdentityRow
          icon={ShieldCheck}
          label="Owned graph"
          value={account.graph_uri}
          mono
        />
        <div className="flex flex-wrap gap-2 border-t pt-5">
          {account.is_member && <Badge variant="secondary">Member</Badge>}
          {account.is_curator && <Badge variant="secondary">Curator</Badge>}
          {account.is_admin && <Badge>Administrator</Badge>}
        </div>
        <p className="text-xs leading-5 text-muted-foreground">
          Username, email, graph identity, and roles are stable account fields.
          An administrator can manage role assignments from the control plane.
        </p>
      </CardContent>
    </Card>
  );
}

function IdentityRow({
  icon: Icon,
  label,
  value,
  mono = false,
}: {
  icon: typeof UserRound;
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex gap-3">
      <Icon
        className="mt-0.5 size-4 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
      <div className="min-w-0">
        <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
          {label}
        </p>
        <p
          className={
            mono
              ? "mt-1 break-all font-mono text-xs"
              : "mt-1 break-words text-sm"
          }
        >
          {value}
        </p>
      </div>
    </div>
  );
}

function MutationMessage({ error }: { error: Error }) {
  const status = (error as PortalApiError).status;
  return (
    <p className="text-sm text-destructive" role="alert">
      {status === 403
        ? "You no longer have permission for this account action."
        : error.message}
    </p>
  );
}

function AccountSkeleton({ compact = false }: { compact?: boolean }) {
  return (
    <div
      className={
        compact
          ? "mt-8 grid gap-6 lg:grid-cols-3"
          : "mx-auto max-w-5xl space-y-6 px-4 py-10 sm:px-6"
      }
      aria-label="Loading account settings"
    >
      {!compact && <Skeleton className="h-10 w-64" />}
      <Skeleton className="h-80 rounded-xl lg:col-span-2" />
      <Skeleton className="h-64 rounded-xl" />
    </div>
  );
}
