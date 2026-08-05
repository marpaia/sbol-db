import { Save } from "lucide-react";
import { useEffect, useState } from "react";

import {
  AdminPage,
  AdminSection,
  Field,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import {
  useAdminInstance,
  useUpdateAdminInstance,
} from "@/features/admin/settings/instance/queries";

interface Draft {
  name: string;
  instance_url: string;
  uri_prefix: string;
  front_page_text: string;
  allow_public_signup: boolean;
  require_login: boolean;
}

const EMPTY: Draft = {
  name: "",
  instance_url: "",
  uri_prefix: "",
  front_page_text: "",
  allow_public_signup: true,
  require_login: false,
};

export default function AdminInstanceRoute() {
  const query = useAdminInstance();
  const update = useUpdateAdminInstance();
  const [draft, setDraft] = useState<Draft>(EMPTY);

  useEffect(() => {
    if (!query.data) return;
    setDraft({
      name: query.data.name,
      instance_url: query.data.instance_url,
      uri_prefix: query.data.uri_prefix,
      front_page_text: query.data.front_page_text,
      allow_public_signup: query.data.allow_public_signup,
      require_login: query.data.require_login,
    });
  }, [query.data]);

  return (
    <AdminPage
      title="Instance settings"
      description="Control the identity and public access policy of this SBOL DB deployment. Product styling remains part of the shared design system, independent of instance branding."
    >
      {query.error ? (
        <SurfaceState
          variant="error"
          title="Instance settings unavailable"
          description={(query.error as Error).message}
        />
      ) : query.isLoading || !query.data ? (
        <SettingsSkeleton />
      ) : (
        <form
          className="space-y-6"
          onSubmit={(event) => {
            event.preventDefault();
            update.mutate(draft);
          }}
        >
          <AdminSection
            title="Registry identity"
            description="These values appear in public navigation, metadata, and newly minted object identifiers."
          >
            <div className="grid gap-5 md:grid-cols-2">
              <Field label="Instance name">
                <Input
                  value={draft.name}
                  onChange={(event) =>
                    setDraft((value) => ({
                      ...value,
                      name: event.target.value,
                    }))
                  }
                  required
                />
              </Field>
              <Field
                label="Public instance URL"
                hint="Leave blank for local or reverse-proxy deployments that do not advertise an origin."
              >
                <Input
                  value={draft.instance_url}
                  onChange={(event) =>
                    setDraft((value) => ({
                      ...value,
                      instance_url: event.target.value,
                    }))
                  }
                  placeholder="https://registry.example.org"
                  inputMode="url"
                />
              </Field>
              <Field
                label="Object URI prefix"
                hint="Changing this affects identifiers minted by future submissions; existing IRIs do not move."
                className="md:col-span-2"
              >
                <Input
                  value={draft.uri_prefix}
                  onChange={(event) =>
                    setDraft((value) => ({
                      ...value,
                      uri_prefix: event.target.value,
                    }))
                  }
                  required
                  inputMode="url"
                />
              </Field>
              <Field label="Front-page introduction" className="md:col-span-2">
                <Textarea
                  value={draft.front_page_text}
                  onChange={(event) =>
                    setDraft((value) => ({
                      ...value,
                      front_page_text: event.target.value,
                    }))
                  }
                  rows={5}
                  placeholder="Describe this registry and its collection policy."
                />
              </Field>
            </div>
          </AdminSection>

          <AdminSection
            title="Public access"
            description="These are enforced server policies, not navigation preferences."
          >
            <div className="divide-y rounded-lg border">
              <PolicyToggle
                title="Allow public account creation"
                description="Visitors may register their own member account."
                checked={draft.allow_public_signup}
                onChange={(checked) =>
                  setDraft((value) => ({
                    ...value,
                    allow_public_signup: checked,
                  }))
                }
              />
              <PolicyToggle
                title="Require sign-in to browse"
                description="Anonymous API and registry browsing is blocked; bootstrap and login remain public."
                checked={draft.require_login}
                onChange={(checked) =>
                  setDraft((value) => ({ ...value, require_login: checked }))
                }
              />
            </div>
          </AdminSection>

          <div className="flex flex-wrap items-center justify-between gap-4">
            <MutationStatus
              pending={update.isPending}
              error={update.error}
              success={update.isSuccess ? "Instance settings saved." : null}
            />
            <Button
              type="submit"
              disabled={update.isPending || !draft.name.trim()}
            >
              <Save /> Save settings
            </Button>
          </div>
        </form>
      )}
    </AdminPage>
  );
}

function PolicyToggle({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-6 px-4 py-4">
      <span>
        <span className="block text-sm font-medium">{title}</span>
        <span className="mt-1 block text-xs leading-5 text-muted-foreground">
          {description}
        </span>
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="size-4 shrink-0 accent-primary"
      />
    </label>
  );
}

function SettingsSkeleton() {
  return (
    <div className="space-y-6">
      <Skeleton className="h-80 rounded-xl" />
      <Skeleton className="h-44 rounded-xl" />
    </div>
  );
}
