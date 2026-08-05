import { useQuery } from "@tanstack/react-query";

import { fetchAdminAudit } from "./api";

export const auditKeys = {
  all: ["admin", "settings", "audit"] as const,
};

export function useAdminAudit() {
  return useQuery({
    queryKey: auditKeys.all,
    queryFn: ({ signal }) => fetchAdminAudit(signal),
  });
}
