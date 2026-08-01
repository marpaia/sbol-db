export const PRODUCT_NAME = "SBOL DB";
export const PRODUCT_TAGLINE = "Biological design infrastructure";

/**
 * A deployment may have a local name, but that name is context within the
 * SBOL DB product rather than a replacement product identity.
 */
export function deploymentName(name: string | null | undefined): string | null {
  const normalized = name?.trim();
  if (
    !normalized ||
    normalized.toLocaleLowerCase() === PRODUCT_NAME.toLocaleLowerCase()
  ) {
    return null;
  }
  return normalized;
}

export function productDocumentTitle(
  instanceName: string | null | undefined
): string {
  const deployment = deploymentName(instanceName);
  return deployment ? `${PRODUCT_NAME} · ${deployment}` : PRODUCT_NAME;
}
