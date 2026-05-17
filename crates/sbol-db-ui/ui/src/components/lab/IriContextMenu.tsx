/**
 * Right-click context menu for IRI cells in the results table.
 * Offers cross-dialect actions (open in SPARQL/SQL, walk neighborhood)
 * plus a Copy IRI shortcut. The menu is positioned at the click point
 * and dismissed on outside-click, Escape, or any action.
 */

import { useEffect } from "react";
import {
  ClipboardCopy,
  ExternalLink,
  GitBranch,
  Database,
  Network,
} from "lucide-react";

import { useLabStore } from "@/lib/store";
import { useNavigate } from "react-router-dom";

export interface IriContextMenuProps {
  x: number;
  y: number;
  iri: string;
  onClose: () => void;
}

export function IriContextMenu({ x, y, iri, onClose }: IriContextMenuProps) {
  const navigate = useNavigate();
  const setBuffer = useLabStore((s) => s.setBuffer);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onClick = () => onClose();
    window.addEventListener("keydown", onKey);
    window.addEventListener("click", onClick);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("click", onClick);
    };
  }, [onClose]);

  const openInSparql = () => {
    const q = `SELECT ?p ?o WHERE {\n  <${iri}> ?p ?o .\n}\nLIMIT 100\n`;
    setBuffer("sparql", q);
    navigate("/sparql");
    onClose();
  };
  const openInSql = () => {
    const escaped = iri.replace(/'/g, "''");
    const q = `SELECT *\nFROM sbol_objects\nWHERE iri = '${escaped}';\n`;
    setBuffer("sql", q);
    navigate("/sql");
    onClose();
  };
  const walkNeighborhood = () => {
    const q = `PREFIX sbol: <http://sbols.org/v3#>\nSELECT ?p ?o\nWHERE {\n  <${iri}> (sbol:|!sbol:)* ?o .\n  <${iri}> ?p ?o .\n}\nLIMIT 100\n`;
    setBuffer("sparql", q);
    navigate("/sparql");
    onClose();
  };
  const openHttp = () => {
    if (/^https?:\/\//i.test(iri)) {
      window.open(iri, "_blank", "noopener");
    }
    onClose();
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(iri);
    } catch {
      // navigator.clipboard can throw in non-secure contexts; ignore
      // and let the user copy from the cell text directly.
    }
    onClose();
  };

  const isHttp = /^https?:\/\//i.test(iri);

  return (
    <div
      role="menu"
      style={{
        position: "fixed",
        top: y,
        left: x,
        zIndex: 100,
      }}
      onClick={(e) => e.stopPropagation()}
      className="min-w-[14rem] rounded-md border bg-popover py-1 text-sm text-popover-foreground shadow-md"
    >
      <div className="truncate border-b px-3 py-1.5 font-mono text-[10px] text-muted-foreground">
        {iri}
      </div>
      <MenuButton
        icon={<ClipboardCopy size={14} />}
        label="Copy IRI"
        onClick={copy}
      />
      <MenuButton
        icon={<Network size={14} />}
        label="Open in SPARQL"
        onClick={openInSparql}
      />
      <MenuButton
        icon={<Database size={14} />}
        label="Open in SQL"
        onClick={openInSql}
      />
      <MenuButton
        icon={<GitBranch size={14} />}
        label="Walk neighborhood"
        onClick={walkNeighborhood}
      />
      {isHttp && (
        <MenuButton
          icon={<ExternalLink size={14} />}
          label="Open in new tab"
          onClick={openHttp}
        />
      )}
    </div>
  );
}

function MenuButton({
  icon,
  label,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="flex w-full items-center gap-2 px-3 py-1.5 text-foreground transition-colors hover:bg-accent"
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}
