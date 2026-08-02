export interface LegalSection {
  id: string;
  title: string;
  paragraphs: string[];
  items?: string[];
}

export function LegalDocument({
  eyebrow,
  title,
  summary,
  effectiveDate,
  noticeTitle,
  notice,
  sections,
}: {
  eyebrow: string;
  title: string;
  summary: string;
  effectiveDate: string;
  noticeTitle: string;
  notice: string;
  sections: LegalSection[];
}) {
  return (
    <div>
      <section className="registry-field border-b border-foreground/15">
        <div className="mx-auto grid max-w-[90rem] lg:grid-cols-[1.05fr_0.95fr]">
          <div className="px-4 py-14 sm:px-6 sm:py-20 lg:border-r lg:border-foreground/15 lg:px-8 lg:py-24 xl:pr-16">
            <p className="ledger-label text-primary">{eyebrow}</p>
            <h1 className="mt-5 max-w-3xl text-balance text-4xl font-medium leading-[1.02] tracking-[-0.04em] sm:text-5xl lg:text-6xl">
              {title}
            </h1>
            <p className="mt-7 max-w-2xl text-pretty text-base leading-7 text-muted-foreground sm:text-lg">
              {summary}
            </p>
            <p className="mt-6 font-mono text-[10px] uppercase tracking-[0.14em] text-muted-foreground">
              Effective {effectiveDate}
            </p>
          </div>

          <div className="flex items-center px-4 py-10 sm:px-6 lg:px-10 lg:py-20 xl:px-14">
            <div className="border border-foreground/15 border-l-2 border-l-primary bg-card p-5 sm:p-6">
              <p className="ledger-label text-primary">{noticeTitle}</p>
              <p className="mt-3 text-sm leading-6 text-muted-foreground">
                {notice}
              </p>
            </div>
          </div>
        </div>
      </section>

      <div className="mx-auto grid max-w-[90rem] gap-10 px-4 py-14 sm:px-6 lg:grid-cols-[0.28fr_0.72fr] lg:gap-16 lg:px-8 lg:py-20">
        <aside className="self-start lg:sticky lg:top-24">
          <p className="ledger-label text-primary">On this page</p>
          <nav
            aria-label={`${title} sections`}
            className="mt-4 border-l border-foreground/15"
          >
            {sections.map((section, index) => (
              <a
                key={section.id}
                href={`#${section.id}`}
                className="grid grid-cols-[2rem_1fr] gap-2 border-b border-foreground/10 py-2.5 pl-3 text-xs leading-5 text-muted-foreground hover:text-foreground"
              >
                <span className="font-mono text-[9px] text-primary">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <span>{section.title}</span>
              </a>
            ))}
          </nav>
        </aside>

        <article className="min-w-0 border-t border-foreground/15">
          {sections.map((section, index) => (
            <section
              key={section.id}
              id={section.id}
              className="scroll-mt-24 border-b border-foreground/15 py-8 sm:py-10"
            >
              <div className="grid gap-4 sm:grid-cols-[3rem_minmax(0,1fr)] sm:gap-6">
                <span className="font-mono text-[10px] tracking-[0.14em] text-primary">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <div className="max-w-3xl">
                  <h2 className="text-2xl font-semibold tracking-[-0.025em]">
                    {section.title}
                  </h2>
                  <div className="mt-4 space-y-4 text-sm leading-7 text-muted-foreground">
                    {section.paragraphs.map((paragraph) => (
                      <p key={paragraph}>{paragraph}</p>
                    ))}
                  </div>
                  {section.items && (
                    <ul className="mt-5 grid gap-2 text-sm leading-6 text-muted-foreground">
                      {section.items.map((item) => (
                        <li key={item} className="flex gap-3">
                          <span
                            className="mt-[0.65rem] size-1 shrink-0 bg-primary"
                            aria-hidden="true"
                          />
                          <span>{item}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              </div>
            </section>
          ))}
        </article>
      </div>
    </div>
  );
}
