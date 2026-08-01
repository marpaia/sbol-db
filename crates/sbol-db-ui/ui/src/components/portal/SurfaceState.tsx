import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { ProductEmptyState } from "@/components/product/ProductEmptyState";

export function SurfaceState({
  title,
  description,
  variant = "empty",
  icon,
  action,
  className,
}: {
  title: string;
  description: string;
  variant?: "empty" | "unsupported" | "error" | "info";
  icon?: LucideIcon;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <ProductEmptyState
      title={title}
      description={description}
      variant={variant}
      icon={icon}
      action={action}
      className={className}
    />
  );
}
