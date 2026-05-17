/**
 * Three-column workbench layout used by both SQL and SPARQL routes:
 *
 *   ┌──────────┬─────────────────────────┬──────────┐
 *   │  schema  │   children (editor +     │  saved + │
 *   │  sidebar │      results)            │  history │
 *   └──────────┴─────────────────────────┴──────────┘
 *
 * The dashboard route deliberately doesn't use this — it renders into
 * `LabLayout`'s Outlet with the full main width, so the schema
 * sidebar isn't distracting on a page that isn't about queries.
 */

import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";

import { QueryHistoryList } from "./QueryHistoryList";
import { SavedQueriesList } from "./SavedQueriesList";
import { SchemaSidebar } from "./SchemaSidebar";
import type { Dialect } from "@/lib/store";

export interface WorkbenchShellProps {
  dialect: Dialect;
  currentBuffer: string;
  onInsertIntoEditor: (text: string) => void;
  onLoadQuery: (query: string) => void;
  children: React.ReactNode;
}

export function WorkbenchShell({
  dialect,
  currentBuffer,
  onInsertIntoEditor,
  onLoadQuery,
  children,
}: WorkbenchShellProps) {
  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={18} minSize={10} maxSize={35}>
        <SchemaSidebar dialect={dialect} onInsert={onInsertIntoEditor} />
      </Panel>
      <PanelResizeHandle className="w-px bg-border transition-colors hover:bg-ring/40" />
      <Panel defaultSize={62} minSize={30}>
        {children}
      </Panel>
      <PanelResizeHandle className="w-px bg-border transition-colors hover:bg-ring/40" />
      <Panel defaultSize={20} minSize={12} maxSize={35}>
        <PanelGroup direction="vertical" className="h-full">
          <Panel defaultSize={50} minSize={20}>
            <SavedQueriesList
              dialect={dialect}
              currentQuery={currentBuffer}
              onLoad={onLoadQuery}
            />
          </Panel>
          <PanelResizeHandle className="h-px bg-border transition-colors hover:bg-ring/40" />
          <Panel defaultSize={50} minSize={20}>
            <QueryHistoryList dialect={dialect} onLoad={onLoadQuery} />
          </Panel>
        </PanelGroup>
      </Panel>
    </PanelGroup>
  );
}
