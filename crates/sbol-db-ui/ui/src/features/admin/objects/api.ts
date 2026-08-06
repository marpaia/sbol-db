export {
  fetchCatalogResource as getObjectByIri,
  fetchCatalogResources as listObjects,
  lookupCatalogResources as lookupObjects,
  type CatalogResourceLookup as LookupObjectsResponse,
  type CatalogResource as SbolObjectRecord,
  type CatalogResourceDetail as ObjectDetail,
  type CatalogResourceQuery as ListObjectsQuery,
} from "@/features/admin/api";
