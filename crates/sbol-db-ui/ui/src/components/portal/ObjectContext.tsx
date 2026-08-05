import { Boxes, GitBranch, Library, Network } from "lucide-react";

import { ObjectRelationGroup } from "@/components/portal/ObjectRelationGroup";
import { ObjectSection } from "@/components/portal/ObjectSection";
import type { PortalObjectDetails } from "@/features/registry/objects/api";

export function ObjectContext({ object }: { object: PortalObjectDetails }) {
  return (
    <ObjectSection
      id="context"
      icon={Network}
      title="Registry context"
      description="Containment, reuse, and exact-sequence relationships visible to this account."
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <ObjectRelationGroup
          icon={Library}
          title="Collections"
          description="Collections that directly contain this object."
          section={object.collections}
          emptyLabel="This object is not a direct member of a visible collection."
        />
        <ObjectRelationGroup
          icon={Boxes}
          title="Members"
          description="Objects directly contained by this Collection."
          section={object.members}
          emptyLabel="This Collection has no direct members."
        />
        <ObjectRelationGroup
          icon={GitBranch}
          title="Used by"
          description="Objects that reference this object directly or one level away."
          section={object.uses}
          emptyLabel="No visible object currently uses this object."
        />
        <ObjectRelationGroup
          icon={Network}
          title="Exact-sequence twins"
          description="Other Components with exactly equal asserted sequence elements."
          section={object.twins}
          emptyLabel="No visible exact-sequence twin was found."
        />
      </div>
    </ObjectSection>
  );
}
