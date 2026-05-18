import { Navigate, Route, Routes, useParams } from "react-router-dom";

import DashboardRoute from "@/routes/DashboardRoute";
import DocumentDetailRoute from "@/routes/DocumentDetailRoute";
import DocumentsRoute from "@/routes/DocumentsRoute";
import JobDetailRoute from "@/routes/JobDetailRoute";
import JobsRoute from "@/routes/JobsRoute";
import LabLayout from "@/routes/LabLayout";
import NeighborhoodRoute from "@/routes/NeighborhoodRoute";
import ObjectDetailRoute from "@/routes/ObjectDetailRoute";
import ObjectLookupRoute from "@/routes/ObjectLookupRoute";
import ObjectsRoute from "@/routes/ObjectsRoute";
import ObservabilityRoute from "@/routes/ObservabilityRoute";
import OntologyDetailRoute from "@/routes/OntologyDetailRoute";
import OntologyRoute from "@/routes/OntologyRoute";
import PgTableDetailRoute from "@/routes/PgTableDetailRoute";
import PostgresRoute from "@/routes/PostgresRoute";
import SchemaRoute from "@/routes/SchemaRoute";
import SequencesRoute from "@/routes/SequencesRoute";
import SparqlRoute from "@/routes/SparqlRoute";
import SqlRoute from "@/routes/SqlRoute";

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<LabLayout />}>
        <Route index element={<DashboardRoute />} />
        <Route path="sparql" element={<SparqlRoute />} />
        <Route path="sql" element={<SqlRoute />} />
        <Route path="schema" element={<SchemaRoute />} />
        <Route path="schema/tables/:name" element={<PgTableDetailRoute />} />
        <Route
          path="schema/tables/:schema/:name"
          element={<RedirectToSchemaTable />}
        />
        <Route path="documents" element={<DocumentsRoute />} />
        <Route path="documents/:id" element={<DocumentDetailRoute />} />
        <Route path="objects" element={<ObjectsRoute />} />
        <Route path="objects/lookup" element={<ObjectLookupRoute />} />
        <Route path="objects/:iri" element={<ObjectDetailRoute />} />
        <Route path="neighborhood" element={<NeighborhoodRoute />} />
        <Route path="sequences" element={<SequencesRoute />} />
        <Route path="ontologies" element={<OntologyRoute />} />
        <Route path="ontologies/:prefix" element={<OntologyDetailRoute />} />
        <Route path="observability" element={<ObservabilityRoute />} />
        <Route path="observability/jobs" element={<JobsRoute />} />
        <Route path="observability/jobs/:id" element={<JobDetailRoute />} />
        <Route path="observability/postgres" element={<PostgresRoute />} />
        <Route
          path="observability/postgres/tables/:schema/:name"
          element={<RedirectToSchemaTable />}
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}

/**
 * Legacy URL: the table detail page used to live under
 * `/observability/postgres/tables/:schema/:name` (and briefly under
 * `/schema/tables/:schema/:name`). The canonical form is now
 * `/schema/tables/:name`; Postgres schemas aren't a domain concept in
 * the UI. Redirect both legacy shapes so old bookmarks keep working.
 */
function RedirectToSchemaTable() {
  const { name } = useParams<{ schema: string; name: string }>();
  return (
    <Navigate to={`/schema/tables/${encodeURIComponent(name ?? "")}`} replace />
  );
}
