import { lazy, Suspense } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";

import ProductIdentity from "@/components/portal/ProductIdentity";
import { Skeleton } from "@/components/ui/skeleton";
import { adminPath } from "@/lib/routes";

const AdminApp = lazy(() => import("@/app/routing/AdminApp"));
const PublicRoutes = lazy(() => import("@/app/routing/PublicRoutes"));

export default function App() {
  return (
    <>
      <ProductIdentity />
      <Routes>
        <Route
          path="admin/*"
          element={
            <Suspense fallback={<AppEntryLoading />}>
              <AdminApp />
            </Suspense>
          }
        />
        <Route path="lab/*" element={<LegacyLabRedirect />} />
        <Route
          path="*"
          element={
            <Suspense fallback={<AppEntryLoading />}>
              <PublicRoutes />
            </Suspense>
          }
        />
      </Routes>
    </>
  );
}

/** Keep bookmarks from the original `/lab` deployment working after cutover. */
function LegacyLabRedirect() {
  const rest = useParams()["*"] || "";
  return <Navigate to={adminPath(rest)} replace />;
}

function AppEntryLoading() {
  return (
    <div className="flex min-h-svh items-center justify-center bg-background">
      <div className="w-full max-w-sm space-y-3 px-6">
        <Skeleton className="h-10 w-10 rounded-xl" />
        <Skeleton className="h-7 w-48" />
        <Skeleton className="h-4 w-full" />
      </div>
    </div>
  );
}
