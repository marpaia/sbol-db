import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import {
  ProductSurface,
  ProductSurfaceBody,
  ProductSurfaceHeader,
} from "@/components/product/ProductSurface";
import { cn } from "@/lib/utils";

export function ObjectSection({
  id,
  icon: Icon,
  title,
  description,
  action,
  children,
  className,
  contentClassName,
}: {
  id?: string;
  icon: LucideIcon;
  title: string;
  description?: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
  contentClassName?: string;
}) {
  return (
    <ProductSurface
      id={id}
      density="comfortable"
      className={cn("scroll-mt-24", className)}
    >
      <ProductSurfaceHeader
        icon={Icon}
        title={title}
        description={description}
        action={action}
      />
      <ProductSurfaceBody className={contentClassName}>
        {children}
      </ProductSurfaceBody>
    </ProductSurface>
  );
}
