import { BackendUnavailable } from "@/components/lab/BackendUnavailable";
import { useBackendInfo } from "@/features/admin/backend/queries";
import { LsmMaintenance, RelationalMaintenance } from "./MaintenancePanels";

/** Select the maintenance experience supported by the active backend. */
export default function MaintenanceRoute() {
  const { data: info } = useBackendInfo();
  const maintenance = info?.capabilities.maintenance ?? null;

  if (info && maintenance === null) {
    return <BackendUnavailable feature="Maintenance" />;
  }

  if (maintenance === "lsm") {
    return <LsmMaintenance />;
  }

  return <RelationalMaintenance capabilities={info?.capabilities} />;
}
