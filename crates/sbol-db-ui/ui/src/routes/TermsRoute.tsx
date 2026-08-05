import {
  LegalDocument,
  type LegalSection,
} from "@/components/portal/LegalDocument";

const EFFECTIVE_DATE = "August 1, 2026";

const sections: LegalSection[] = [
  {
    id: "agreement",
    title: "Agreement and operator",
    paragraphs: [
      "These Terms govern use of the SBOL DB website, registry, account workspace, APIs, and related services provided by the person or organization operating this deployment (the Operator). By creating an account or using a non-public service feature, you agree to these Terms and any deployment-specific terms presented by the Operator.",
      "SBOL DB is open-source software used by independent operators. Upstream contributors are not parties to these Terms merely because they wrote or maintain the software. The Operator must identify the legal entity or person providing this service before enabling public registration.",
    ],
  },
  {
    id: "eligibility",
    title: "Eligibility and accounts",
    paragraphs: [
      "You must be legally able to agree to these Terms. If you use the service for an organization, you represent that you are authorized to act for it. Users who are not old enough to enter a binding agreement may use the service only through an authorized institution, parent, or guardian where permitted.",
      "Provide accurate account information, protect your credentials, and promptly report suspected compromise. You are responsible for activity performed through your account, API credentials, and OAuth grants unless you have reported unauthorized use to the Operator.",
    ],
  },
  {
    id: "service",
    title: "Using the registry",
    paragraphs: [
      "You may use the service to store, validate, search, exchange, synchronize, share, review, and publish biological designs in accordance with your permissions and applicable law. Features and limits may differ by deployment, account role, storage backend, or integration.",
      "Registry results, validation, conversion, visualization, search ranking, and sequence analysis are technical aids. You remain responsible for scientific review, biosafety assessment, regulatory compliance, and decisions based on the service.",
    ],
  },
  {
    id: "content",
    title: "Your content",
    paragraphs: [
      "You retain the rights you hold in designs and other material you submit. You grant the Operator a non-exclusive, worldwide, royalty-free license to host, reproduce, validate, index, convert between supported representations, display, transmit, back up, and otherwise process that material only as needed to operate, secure, and improve the service and to follow the sharing or publication choices you make.",
      "You represent that you have the rights and permissions needed to submit the content and associated personal data. Do not remove or falsify authorship, provenance, licensing, safety, or attribution information.",
    ],
  },
  {
    id: "publication",
    title: "Publication and persistent identity",
    paragraphs: [
      "Publishing a design makes the selected content and metadata available to the public. Persistent design identifiers, versions, provenance, citations, and content fingerprints are intended to support durable scientific references. Publication may therefore be difficult to reverse after other people or systems have downloaded, cited, cached, or synchronized the record.",
      "Publication does not automatically grant a separate intellectual-property license unless the interface or associated metadata expressly identifies one. You are responsible for choosing and recording an appropriate license and for confirming that publication does not violate confidentiality, contractual, export-control, biosafety, or other obligations.",
    ],
  },
  {
    id: "collaboration",
    title: "Sharing and delegated access",
    paragraphs: [
      "You may grant other users access to private designs and authorize applications or agents to act through OAuth. Review the recipient, application, requested scopes, and intended effect before approving access. Scopes are additive permissions and remain bounded by your current rights to each design.",
      "Prepared actions, dry runs, validation, or review screens reduce risk but do not transfer responsibility for an approved change. Revoke access that is no longer needed and review unexpected account or audit activity promptly.",
    ],
  },
  {
    id: "automation",
    title: "APIs, tools, and agents",
    paragraphs: [
      "Automated access must use documented interfaces, respect authentication and rate limits, preserve attribution and provenance, and remain within approved scopes. You are responsible for configuring and supervising tools or agents connected through your account.",
      "Do not attempt to bypass permissions, replay credentials across audiences, evade safety limits, scrape private data, or cause automated activity that degrades the service for others. The Operator may limit or disable abusive or unsafe integrations.",
    ],
  },
  {
    id: "acceptable-use",
    title: "Acceptable use",
    paragraphs: [
      "Use the service responsibly and lawfully. You must not interfere with the registry, compromise another account, misrepresent identity or provenance, upload malicious code, infringe privacy or intellectual-property rights, or use the service to facilitate unlawful or intentionally harmful biological activity.",
    ],
    items: [
      "Do not probe, disrupt, overload, or circumvent the service or its security controls except through an authorized security-testing program.",
      "Do not submit content you are prohibited from possessing, sharing, exporting, or publishing.",
      "Do not use another person's identity, credentials, private designs, or delegated authority without permission.",
      "Do not rely on the service as the sole control for laboratory safety, clinical decisions, regulatory approval, or containment of hazardous work.",
    ],
  },
  {
    id: "third-party",
    title: "Third-party services and software",
    paragraphs: [
      "The service may interoperate with external identity providers, ontologies, repositories, applications, agents, hosting providers, or other services. Their terms and privacy practices may apply separately, and the Operator is not responsible for a third party outside its control.",
      "The SBOL DB source code and bundled open-source components remain governed by their respective software licenses. These service Terms do not replace or restrict rights granted under those licenses.",
    ],
  },
  {
    id: "availability",
    title: "Availability and changes",
    paragraphs: [
      "The Operator may change, suspend, limit, or discontinue features; establish reasonable usage limits; and perform maintenance. Where practical, the Operator should provide notice of material changes or planned discontinuation and a reasonable opportunity to export accessible content.",
      "You are responsible for keeping appropriate copies of important designs and records. Registry export or backup features may not include every account, credential, job, attachment, or operational record.",
    ],
  },
  {
    id: "disclaimers",
    title: "Disclaimers",
    paragraphs: [
      "To the maximum extent permitted by law, the service is provided as available and without warranties of accuracy, completeness, fitness for a particular purpose, non-infringement, uninterrupted operation, or preservation of every record. Scientific and biological information may be incomplete, incorrect, outdated, or unsafe for a proposed use.",
      "Nothing in the service constitutes medical, clinical, legal, regulatory, or biosafety advice. Some jurisdictions do not allow particular warranty exclusions, so parts of this section may not apply to you.",
    ],
  },
  {
    id: "liability",
    title: "Limitation of liability",
    paragraphs: [
      "To the maximum extent permitted by law, the Operator and upstream SBOL DB contributors will not be liable for indirect, incidental, special, consequential, exemplary, or punitive damages, or for loss of data, research time, revenue, goodwill, or opportunity arising from use of the service. Any deployment-specific cap on direct liability must be stated by the Operator.",
      "This limitation does not exclude liability that cannot lawfully be limited, and it does not alter rights that applicable consumer law makes non-waivable.",
    ],
  },
  {
    id: "suspension",
    title: "Suspension and termination",
    paragraphs: [
      "The Operator may suspend or terminate access when reasonably necessary to protect the service or others, investigate misuse, comply with law, address nonpayment where applicable, or enforce these Terms. When circumstances permit, the Operator should provide notice and an opportunity to correct the issue or export accessible content.",
      "Account termination does not necessarily remove published designs, transferred content, citations, or audit records. Provisions that by their nature should survive termination, including content licenses already exercised, publication consequences, disclaimers, and liability limits, will continue to apply.",
    ],
  },
  {
    id: "changes-contact",
    title: "Changes, governing terms, and contact",
    paragraphs: [
      "The Operator may update these Terms. Material changes should be announced before taking effect. Continued use of non-public service features after the effective date constitutes acceptance where permitted by law; if you do not agree, stop using those features and request account closure.",
      "The Operator must publish its identity, contact information, governing law, dispute process, and any deployment-specific commercial or institutional terms. Questions about these Terms should be directed to that Operator. If those details are not available, contact the organization that provided your account before submitting content or relying on the service.",
    ],
  },
];

export default function TermsRoute() {
  return (
    <LegalDocument
      eyebrow="Terms"
      title="Terms of Use"
      summary="The responsibilities that apply when you maintain an account, contribute designs, collaborate, publish records, or connect tools and agents."
      effectiveDate={EFFECTIVE_DATE}
      noticeTitle="Draft for this deployment"
      notice="These terms are a deployment-ready draft, not a substitute for operator review. Before public registration is enabled, the operator should identify itself and add its contact information, governing law, dispute process, service limits, and any institutional or commercial requirements."
      sections={sections}
    />
  );
}
