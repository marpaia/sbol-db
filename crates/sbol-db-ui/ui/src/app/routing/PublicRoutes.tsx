import { lazy, Suspense } from "react";
import type { ReactNode } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";

import PublicShell from "@/components/portal/PublicShell";
import { Skeleton } from "@/components/ui/skeleton";
import SetupRoute from "@/features/instance/SetupRoute";
import { useInstance } from "@/features/instance/queries";
import HomeRoute from "@/features/registry/discovery/HomeRoute";
import LoginRoute from "@/features/session/LoginRoute";
import RegisterRoute from "@/features/session/RegisterRoute";
import { publicObjectPath } from "@/lib/routes";
import NotFoundRoute from "@/routes/NotFoundRoute";
import PublicAccessGate from "@/routes/PublicAccessGate";

const PublicObjectRoute = lazy(
  () => import("@/features/registry/objects/PublicObjectRoute")
);
const AboutRoute = lazy(() => import("@/routes/AboutRoute"));
const PrivacyRoute = lazy(() => import("@/routes/PrivacyRoute"));
const TermsRoute = lazy(() => import("@/routes/TermsRoute"));
const ContributionRoute = lazy(
  () => import("@/features/registry/contributions/ContributionRoute")
);
const SearchRoute = lazy(
  () => import("@/features/registry/discovery/SearchRoute")
);
const SequenceSearchRoute = lazy(
  () => import("@/features/registry/discovery/SequenceSearchRoute")
);
const WorkspaceRoute = lazy(
  () => import("@/features/registry/workspace/WorkspaceRoute")
);
const AccountRoute = lazy(
  () => import("@/features/registry/account/AccountRoute")
);
export default function PublicRoutes() {
  return (
    <Routes>
      <Route element={<PublicShell />}>
        <Route path="login" element={<LoginRoute />} />
        <Route path="register" element={<RegisterRoute />} />
        <Route path="setup" element={<SetupRoute />} />
        <Route path="about" element={<AboutRoute />} />
        <Route path="privacy" element={<PrivacyRoute />} />
        <Route path="terms" element={<TermsRoute />} />
        <Route element={<PublicAccessGate />}>
          <Route index element={<HomeRoute />} />
          <Route
            path="search/*"
            element={
              <PortalRoute>
                <SearchRoute />
              </PortalRoute>
            }
          />
          <Route
            path="sequence-search"
            element={
              <PortalRoute>
                <SequenceSearchRoute />
              </PortalRoute>
            }
          />
          <Route
            path="contribute"
            element={
              <PortalRoute>
                <ContributionRoute />
              </PortalRoute>
            }
          />
          <Route
            path="submit"
            element={<Navigate to="/contribute" replace />}
          />
          <Route
            path="workspace/*"
            element={
              <PortalRoute>
                <WorkspaceRoute />
              </PortalRoute>
            }
          />
          <Route
            path="account"
            element={
              <PortalRoute>
                <AccountRoute />
              </PortalRoute>
            }
          />
          <Route
            path="advanced-search"
            element={<Navigate to="/search" replace />}
          />
          <Route
            path="objects/view/:iri"
            element={
              <PortalRoute>
                <PublicObjectRoute />
              </PortalRoute>
            }
          />
          <Route
            path="public/:collectionId/:displayId/:version?"
            element={<CanonicalObjectRedirect scope="public" />}
          />
          <Route
            path="user/:userId/:collectionId/:displayId/:version?"
            element={<CanonicalObjectRedirect scope="user" />}
          />
        </Route>
        <Route path="*" element={<NotFoundRoute />} />
      </Route>
    </Routes>
  );
}

function PortalRoute({ children }: { children: ReactNode }) {
  return <Suspense fallback={<PortalEntryLoading />}>{children}</Suspense>;
}

function PortalEntryLoading() {
  return (
    <div
      className="mx-auto w-full max-w-7xl space-y-5 px-4 py-12 sm:px-6 lg:px-8"
      aria-label="Loading page"
    >
      <Skeleton className="h-5 w-28" />
      <Skeleton className="h-10 w-full max-w-xl" />
      <Skeleton className="h-24 w-full rounded-xl" />
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        <Skeleton className="h-48 rounded-xl" />
        <Skeleton className="h-48 rounded-xl" />
        <Skeleton className="h-48 rounded-xl" />
      </div>
    </div>
  );
}

function CanonicalObjectRedirect({ scope }: { scope: "public" | "user" }) {
  const instance = useInstance();
  const params = useParams<{
    userId?: string;
    collectionId: string;
    displayId: string;
    version?: string;
  }>();
  if (!instance.data) return null;

  const prefix = instance.data.uri_prefix.replace(/\/+$/, "");
  const segments =
    scope === "public"
      ? ["public", params.collectionId, params.displayId, params.version]
      : [
          "user",
          params.userId,
          params.collectionId,
          params.displayId,
          params.version,
        ];
  const iri = `${prefix}/${segments.filter(Boolean).join("/")}`;
  return <Navigate to={publicObjectPath(iri)} replace />;
}
