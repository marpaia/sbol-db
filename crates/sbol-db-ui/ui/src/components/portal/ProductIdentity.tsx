import { useEffect } from "react";

import { useInstance } from "@/features/portal/queries";
import { productDocumentTitle } from "@/lib/product";

/** Keep browser metadata aligned with the fixed SBOL DB product identity. */
export default function ProductIdentity() {
  const instance = useInstance();

  useEffect(() => {
    document.title = productDocumentTitle(instance.data?.name);
  }, [instance.data?.name]);

  return null;
}
