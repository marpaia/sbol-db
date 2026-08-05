import { Plus, ShieldCheck, Trash2, UserRound } from "lucide-react";
import { useEffect, useState } from "react";

import {
  AdminPage,
  AdminSection,
  Field,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import type {
  AdminUser,
  CreateAdminUser,
} from "@/features/admin/settings/users/api";
import {
  useAdminUsers,
  useCreateAdminUser,
  useDeleteAdminUser,
  useUpdateAdminUser,
} from "@/features/admin/settings/users/queries";
import { useSession } from "@/features/session/queries";

const EMPTY_USER: CreateAdminUser = {
  username: "",
  name: "",
  email: "",
  affiliation: "",
  password: "",
  is_admin: false,
  is_curator: false,
  is_member: true,
};

export default function AdminUsersRoute() {
  const query = useAdminUsers();
  const create = useCreateAdminUser();
  const update = useUpdateAdminUser();
  const remove = useDeleteAdminUser();
  const session = useSession();
  const [draft, setDraft] = useState<CreateAdminUser>(EMPTY_USER);

  return (
    <AdminPage
      title="Users and roles"
      description="Manage local accounts and explicit member, curator, and administrator roles. Password hashes, reset links, and API tokens are never returned to this interface."
    >
      <AdminSection
        title="Create account"
        description="Create an account directly when public registration is disabled or an elevated role is required."
      >
        <form
          className="grid gap-4 md:grid-cols-2"
          onSubmit={(event) => {
            event.preventDefault();
            create.mutate(draft, {
              onSuccess: () => setDraft(EMPTY_USER),
            });
          }}
        >
          <Field label="Username">
            <Input
              value={draft.username}
              onChange={(event) =>
                setDraft((value) => ({
                  ...value,
                  username: event.target.value,
                }))
              }
              autoComplete="off"
              required
            />
          </Field>
          <Field label="Display name">
            <Input
              value={draft.name}
              onChange={(event) =>
                setDraft((value) => ({ ...value, name: event.target.value }))
              }
              required
            />
          </Field>
          <Field label="Email">
            <Input
              type="email"
              value={draft.email}
              onChange={(event) =>
                setDraft((value) => ({ ...value, email: event.target.value }))
              }
              required
            />
          </Field>
          <Field label="Affiliation">
            <Input
              value={draft.affiliation ?? ""}
              onChange={(event) =>
                setDraft((value) => ({
                  ...value,
                  affiliation: event.target.value,
                }))
              }
            />
          </Field>
          <Field
            label="Temporary password"
            hint="The password is sent once and never displayed again."
          >
            <Input
              type="password"
              value={draft.password}
              onChange={(event) =>
                setDraft((value) => ({
                  ...value,
                  password: event.target.value,
                }))
              }
              autoComplete="new-password"
              required
            />
          </Field>
          <div className="flex flex-wrap items-end gap-4 pb-1">
            <RoleCheckbox
              label="Member"
              checked={draft.is_member}
              onChange={(checked) =>
                setDraft((value) => ({ ...value, is_member: checked }))
              }
            />
            <RoleCheckbox
              label="Curator"
              checked={draft.is_curator}
              onChange={(checked) =>
                setDraft((value) => ({ ...value, is_curator: checked }))
              }
            />
            <RoleCheckbox
              label="Administrator"
              checked={draft.is_admin}
              onChange={(checked) =>
                setDraft((value) => ({ ...value, is_admin: checked }))
              }
            />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-3 md:col-span-2">
            <MutationStatus
              pending={create.isPending}
              error={create.error}
              success={create.isSuccess ? "Account created." : null}
            />
            <Button type="submit" disabled={create.isPending}>
              <Plus /> Create account
            </Button>
          </div>
        </form>
      </AdminSection>

      <AdminSection
        title="Accounts"
        description={
          query.data
            ? `${query.data.total.toLocaleString()} local account${query.data.total === 1 ? "" : "s"}`
            : "Local account directory"
        }
      >
        {query.error ? (
          <SurfaceState
            variant="error"
            title="Accounts unavailable"
            description={(query.error as Error).message}
          />
        ) : query.isLoading || !query.data ? (
          <div className="space-y-3">
            <Skeleton className="h-48 rounded-lg" />
            <Skeleton className="h-48 rounded-lg" />
          </div>
        ) : query.data.items.length === 0 ? (
          <SurfaceState
            title="No accounts"
            description="Create the first local account above."
          />
        ) : (
          <div className="space-y-3">
            {query.data.items.map((user) => (
              <UserCard
                key={user.id}
                user={user}
                current={session.data?.user?.id === user.id}
                update={update}
                remove={remove}
              />
            ))}
          </div>
        )}
      </AdminSection>
    </AdminPage>
  );
}

function UserCard({
  user,
  current,
  update,
  remove,
}: {
  user: AdminUser;
  current: boolean;
  update: ReturnType<typeof useUpdateAdminUser>;
  remove: ReturnType<typeof useDeleteAdminUser>;
}) {
  const [draft, setDraft] = useState({
    name: user.name,
    email: user.email,
    affiliation: user.affiliation ?? "",
    is_admin: user.is_admin,
    is_curator: user.is_curator,
    is_member: user.is_member,
  });
  const [confirming, setConfirming] = useState(false);
  const [confirmation, setConfirmation] = useState("");

  useEffect(() => {
    setDraft({
      name: user.name,
      email: user.email,
      affiliation: user.affiliation ?? "",
      is_admin: user.is_admin,
      is_curator: user.is_curator,
      is_member: user.is_member,
    });
  }, [user]);

  const updatingThis =
    update.isPending && update.variables?.username === user.username;
  const deletingThis =
    remove.isPending && remove.variables?.username === user.username;

  return (
    <article className="rounded-lg border bg-background p-4 sm:p-5">
      <header className="flex flex-wrap items-start gap-3">
        <span className="flex size-9 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          {user.is_admin ? (
            <ShieldCheck className="size-4" />
          ) : (
            <UserRound className="size-4" />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-medium">{user.name}</h3>
            <code className="text-xs text-muted-foreground">
              @{user.username}
            </code>
            {current && <Badge variant="secondary">Current account</Badge>}
          </div>
          <p className="mt-1 truncate text-xs text-muted-foreground">
            {user.graph_uri}
          </p>
        </div>
      </header>

      <div className="mt-5 grid gap-4 md:grid-cols-3">
        <Field label="Display name">
          <Input
            value={draft.name}
            onChange={(event) =>
              setDraft((value) => ({ ...value, name: event.target.value }))
            }
          />
        </Field>
        <Field label="Email">
          <Input
            type="email"
            value={draft.email}
            onChange={(event) =>
              setDraft((value) => ({ ...value, email: event.target.value }))
            }
          />
        </Field>
        <Field label="Affiliation">
          <Input
            value={draft.affiliation}
            onChange={(event) =>
              setDraft((value) => ({
                ...value,
                affiliation: event.target.value,
              }))
            }
          />
        </Field>
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-4">
        <RoleCheckbox
          label="Member"
          checked={draft.is_member}
          onChange={(checked) =>
            setDraft((value) => ({ ...value, is_member: checked }))
          }
        />
        <RoleCheckbox
          label="Curator"
          checked={draft.is_curator}
          onChange={(checked) =>
            setDraft((value) => ({ ...value, is_curator: checked }))
          }
        />
        <RoleCheckbox
          label="Administrator"
          checked={draft.is_admin}
          disabled={current && user.is_admin}
          onChange={(checked) =>
            setDraft((value) => ({ ...value, is_admin: checked }))
          }
        />
        <Button
          type="button"
          size="sm"
          className="ml-auto"
          disabled={updatingThis}
          onClick={() =>
            update.mutate({ username: user.username, payload: draft })
          }
        >
          Save account
        </Button>
      </div>

      {update.variables?.username === user.username && (
        <div className="mt-3">
          <MutationStatus
            pending={updatingThis}
            error={update.error}
            success={update.isSuccess ? "Account updated." : null}
          />
        </div>
      )}

      <div className="mt-5 border-t pt-4">
        {!confirming ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            disabled={current}
            onClick={() => setConfirming(true)}
          >
            <Trash2 />{" "}
            {current ? "Current account cannot be deleted" : "Delete account"}
          </Button>
        ) : (
          <div className="rounded-lg border border-destructive/25 bg-destructive/5 p-4">
            <p className="text-xs leading-5 text-muted-foreground">
              Type{" "}
              <code className="font-semibold text-foreground">
                DELETE {user.username}
              </code>{" "}
              to remove this account. Objects are not deleted or silently
              reassigned.
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <Input
                value={confirmation}
                onChange={(event) => setConfirmation(event.target.value)}
                className="max-w-sm bg-background font-mono"
                aria-label="Delete confirmation"
              />
              <Button
                type="button"
                variant="destructive"
                disabled={
                  deletingThis || confirmation !== `DELETE ${user.username}`
                }
                onClick={() =>
                  remove.mutate(
                    { username: user.username, confirmation },
                    { onSuccess: () => setConfirming(false) }
                  )
                }
              >
                <Trash2 /> Delete permanently
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setConfirming(false)}
              >
                Cancel
              </Button>
            </div>
            <div className="mt-3">
              <MutationStatus pending={deletingThis} error={remove.error} />
            </div>
          </div>
        )}
      </div>
    </article>
  );
}

function RoleCheckbox({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="inline-flex items-center gap-2 text-xs font-medium">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="size-4 accent-primary"
      />
      {label}
    </label>
  );
}
