import { MachineAccessSection } from "@/components/portal/MachineAccessSection";
import { useInstance } from "@/features/portal/queries";

export default function ConnectRoute() {
  const instance = useInstance();

  return (
    <MachineAccessSection
      mcpServerAddress={instance.data?.machine_access?.mcp_url}
    />
  );
}
