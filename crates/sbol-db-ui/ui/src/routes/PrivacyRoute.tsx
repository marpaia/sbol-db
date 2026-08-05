import {
  LegalDocument,
  type LegalSection,
} from "@/components/portal/LegalDocument";

const EFFECTIVE_DATE = "August 1, 2026";

const sections: LegalSection[] = [
  {
    id: "scope",
    title: "Scope and operator",
    paragraphs: [
      "This policy applies to the SBOL DB website, registry, account workspace, APIs, and related services made available by the person or organization operating this deployment (the Operator). The Operator determines why and how personal data is processed for this instance.",
      "SBOL DB is also open-source software that can be deployed by independent organizations. Upstream contributors do not automatically receive or control data held by this instance. The Operator must identify itself and provide a privacy contact before enabling public account registration.",
    ],
  },
  {
    id: "data",
    title: "Information we process",
    paragraphs: [
      "The information processed depends on how you use the registry. Public browsing generally requires less information than maintaining an account, collaborating on private designs, or authorizing another application.",
    ],
    items: [
      "Account information, including username, name, email address, affiliation, account roles, timestamps, and the graph identity associated with the account.",
      "Authentication information, including password hashes, browser-session records, password-reset records when enabled, API credentials, and OAuth clients, grants, scopes, and token records.",
      "Registry content, including uploaded designs, sequences, collections, identifiers, provenance, citations, annotations, validation results, and publication state.",
      "Collaboration and operational records, including sharing, ownership, review, prepared-action, job, synchronization, and audit events.",
      "Technical records that the application or deployment infrastructure may generate, such as request time, network address, user agent, error details, and security or performance logs.",
    ],
  },
  {
    id: "use",
    title: "How information is used",
    paragraphs: [
      "The Operator processes information to provide and secure the registry, authenticate users, enforce permissions, preserve design identity, perform requested searches and conversions, support collaboration, operate integrations, diagnose failures, maintain audit evidence, and comply with applicable law.",
      "Personal data should not be used for advertising, sold, or used to train unrelated machine-learning models unless the Operator provides a separate, clear notice and obtains any consent required by law.",
    ],
  },
  {
    id: "cookies",
    title: "Sessions and local storage",
    paragraphs: [
      "SBOL DB uses a strictly necessary, same-origin browser-session cookie when you sign in. The cookie carries an opaque session credential and is configured to be unavailable to browser scripts. Theme or interface preferences may be stored locally in your browser.",
      "A deployment may place SBOL DB behind hosting, security, analytics, or observability services that process additional technical data. The Operator is responsible for identifying those services and any non-essential cookies in a deployment-specific notice.",
    ],
  },
  {
    id: "content",
    title: "Public and private registry content",
    paragraphs: [
      "Private content is available only through the account, collection, and design permissions enforced by the registry. Authorized collaborators, curators, and administrators may access it as required by their roles.",
      "When you publish a design, its content, identity, metadata, provenance, and relationships may become publicly searchable and downloadable. Public identifiers and published copies may persist in citations, exports, caches, synchronized workspaces, or third-party systems even if the Operator later changes or removes the registry record.",
    ],
  },
  {
    id: "sharing",
    title: "When information is shared",
    paragraphs: [
      "Information may be shared with people you authorize, registry curators and administrators, applications or agents you approve through OAuth, and vendors that process data for the Operator under appropriate instructions. Information may also be disclosed when required by law, to protect the service or its users, or as part of an organizational transaction subject to applicable safeguards.",
      "OAuth authorization screens identify the requesting application and scopes. A scope permits a category of action but does not override the current ownership or access-control rules for a design.",
    ],
  },
  {
    id: "retention",
    title: "Retention and deletion",
    paragraphs: [
      "The Operator should retain account, session, integration, content, job, and log data only for as long as needed for the purposes described here, the integrity of the scientific record, security, backups, dispute resolution, and legal obligations. Session and OAuth records may expire or be revoked on schedules configured by the deployment.",
      "Deleting an account does not necessarily delete designs that were transferred, shared, cited, or published, and it may not remove append-only audit evidence needed to explain prior registry actions. The Operator should document any deployment-specific retention periods and backup deletion schedule.",
    ],
  },
  {
    id: "security",
    title: "Security",
    paragraphs: [
      "SBOL DB provides controls including password hashing, same-origin browser sessions, scoped authorization, record-level permissions, validation, and audit records. The Operator is responsible for secure hosting, transport encryption, secrets, backups, software updates, access reviews, incident response, and vendor management.",
      "No service can guarantee absolute security. Report a suspected vulnerability or unauthorized disclosure to the Operator rather than placing secrets or personal information in a public issue.",
    ],
  },
  {
    id: "rights",
    title: "Your choices and rights",
    paragraphs: [
      "Depending on your location, you may have rights to access, correct, export, delete, restrict, or object to certain processing of personal data, and to withdraw consent where processing relies on consent. You may also be entitled to complain to a data-protection authority.",
      "Account profile controls allow some information to be reviewed or changed directly. Other requests, including account deletion, OAuth revocation, or a copy of personal data, should be directed to the Operator. A request may be limited where retention is required for security, legal obligations, or the integrity of published and audited records.",
    ],
  },
  {
    id: "transfers",
    title: "International processing",
    paragraphs: [
      "The Operator and its service providers may process information in countries other than your own. The Operator is responsible for identifying where data is processed and for implementing any contractual or legal safeguards required for international transfers.",
    ],
  },
  {
    id: "children",
    title: "Children",
    paragraphs: [
      "SBOL DB is intended for research, education, engineering, and registry administration. It is not directed to children under 13, and the Operator should not knowingly collect personal data from a child without the authorization and safeguards required by applicable law.",
    ],
  },
  {
    id: "changes-contact",
    title: "Changes and contact",
    paragraphs: [
      "The Operator may update this policy as the deployment, its vendors, or applicable requirements change. Material changes should be announced through the service before they take effect, and the effective date should be updated.",
      "Questions, requests, and complaints should be sent to the privacy contact published by the Operator for this deployment. If no operator identity and contact method are published, contact the organization that provided your account before submitting personal or confidential information.",
    ],
  },
];

export default function PrivacyRoute() {
  return (
    <LegalDocument
      eyebrow="Privacy"
      title="Privacy Policy"
      summary="How an SBOL DB deployment handles account information, registry content, authentication, collaboration records, and technical data."
      effectiveDate={EFFECTIVE_DATE}
      noticeTitle="Deployment responsibility"
      notice="SBOL DB can be operated independently by many organizations. The operator of this instance, not the upstream open-source contributors, is responsible for identifying itself, publishing contact details, and adapting this draft to its actual hosting, vendors, retention periods, and legal obligations."
      sections={sections}
    />
  );
}
