import { useMemo } from "react";
import { Navigate, useParams } from "react-router-dom";

import { translateClassicSearchPath } from "./discovery";
import { SearchExperience } from "./SearchExperience";

/** Canonicalize classic search URLs before opening the discovery experience. */
export default function SearchRoute() {
  const classicPath = useParams()["*"]?.trim() || "";
  const classicLocation = useMemo(
    () => (classicPath ? translateClassicSearchPath(classicPath) : null),
    [classicPath]
  );

  if (!classicPath) return <SearchExperience />;

  return (
    <Navigate
      to={{
        pathname: classicLocation?.pathname || "/search",
        search: classicLocation?.params.toString() || "",
      }}
      replace
    />
  );
}
