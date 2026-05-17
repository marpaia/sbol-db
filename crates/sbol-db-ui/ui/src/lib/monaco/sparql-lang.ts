/**
 * SPARQL 1.1 language definition for Monaco.
 *
 * Provides a Monarch tokenizer (so the editor gets reasonable syntax
 * highlighting), the standard SPARQL keyword list for completion, and
 * a configurable prefix completion source. The server-driven
 * validation hook plugs in via `monaco.languages.registerCodeLensProvider`
 * etc. in a later PR; this file just sets up the language so the editor
 * has something to render right away.
 */

import type * as MonacoNS from "monaco-editor";

export const SPARQL_LANGUAGE_ID = "sparql";

const SPARQL_KEYWORDS = [
  "BASE",
  "PREFIX",
  "SELECT",
  "DISTINCT",
  "REDUCED",
  "CONSTRUCT",
  "DESCRIBE",
  "ASK",
  "FROM",
  "NAMED",
  "WHERE",
  "GROUP",
  "BY",
  "HAVING",
  "ORDER",
  "ASC",
  "DESC",
  "LIMIT",
  "OFFSET",
  "VALUES",
  "OPTIONAL",
  "UNION",
  "MINUS",
  "FILTER",
  "BIND",
  "AS",
  "SERVICE",
  "SILENT",
  "GRAPH",
  "EXISTS",
  "NOT",
  "IN",
  "A",
  "TRUE",
  "FALSE",
];

const SPARQL_FUNCTIONS = [
  "STR",
  "LANG",
  "LANGMATCHES",
  "DATATYPE",
  "BOUND",
  "IRI",
  "URI",
  "BNODE",
  "RAND",
  "ABS",
  "CEIL",
  "FLOOR",
  "ROUND",
  "CONCAT",
  "STRLEN",
  "UCASE",
  "LCASE",
  "ENCODE_FOR_URI",
  "CONTAINS",
  "STRSTARTS",
  "STRENDS",
  "STRBEFORE",
  "STRAFTER",
  "REPLACE",
  "REGEX",
  "SUBSTR",
  "YEAR",
  "MONTH",
  "DAY",
  "HOURS",
  "MINUTES",
  "SECONDS",
  "TIMEZONE",
  "TZ",
  "NOW",
  "UUID",
  "STRUUID",
  "MD5",
  "SHA1",
  "SHA256",
  "SHA384",
  "SHA512",
  "COALESCE",
  "IF",
  "STRLANG",
  "STRDT",
  "SAMETERM",
  "ISIRI",
  "ISURI",
  "ISBLANK",
  "ISLITERAL",
  "ISNUMERIC",
  "COUNT",
  "SUM",
  "MIN",
  "MAX",
  "AVG",
  "SAMPLE",
  "GROUP_CONCAT",
];

const SPARQL_PREFIXES_DEFAULT: { prefix: string; iri: string }[] = [
  { prefix: "sbol", iri: "http://sbols.org/v3#" },
  { prefix: "prov", iri: "http://www.w3.org/ns/prov#" },
  {
    prefix: "om",
    iri: "http://www.ontology-of-units-of-measure.org/resource/om-2/",
  },
  { prefix: "rdf", iri: "http://www.w3.org/1999/02/22-rdf-syntax-ns#" },
  { prefix: "rdfs", iri: "http://www.w3.org/2000/01/rdf-schema#" },
  { prefix: "owl", iri: "http://www.w3.org/2002/07/owl#" },
  { prefix: "xsd", iri: "http://www.w3.org/2001/XMLSchema#" },
  { prefix: "so", iri: "http://purl.obolibrary.org/obo/SO_" },
  { prefix: "sbo", iri: "http://biomodels.net/SBO/SBO_" },
];

export function registerSparql(monaco: typeof MonacoNS): void {
  const existing = monaco.languages
    .getLanguages()
    .find((l) => l.id === SPARQL_LANGUAGE_ID);
  if (existing) return;

  monaco.languages.register({
    id: SPARQL_LANGUAGE_ID,
    extensions: [".rq", ".sparql"],
    aliases: ["SPARQL", "sparql"],
  });

  monaco.languages.setMonarchTokensProvider(SPARQL_LANGUAGE_ID, {
    ignoreCase: true,
    keywords: SPARQL_KEYWORDS,
    builtinFunctions: SPARQL_FUNCTIONS,
    tokenizer: {
      root: [
        // Comments — '#' to end of line.
        [/#.*$/, "comment"],
        // Variables: ?var, $var
        [/[?$][A-Za-z_][\w]*/, "variable"],
        // Full IRIs: <http://example/>
        [/<[^>\s]*>/, "iri"],
        // Prefixed names: prefix:local
        [/[A-Za-z_][\w.-]*:[A-Za-z_][\w.-]*/, "pname"],
        // Numbers
        [/-?\d+(\.\d+)?([eE][+-]?\d+)?/, "number"],
        // Triple-quoted strings
        [/"""/, { token: "string.quote", next: "@stringTriple" }],
        [/'''/, { token: "string.quote", next: "@stringTripleSingle" }],
        // Single-line strings
        [/"([^"\\]|\\.)*"/, "string"],
        [/'([^'\\]|\\.)*'/, "string"],
        // Punctuation
        [/[{}()[\]]/, "delimiter.bracket"],
        [/[,;.]/, "delimiter"],
        // Keywords / functions / identifiers
        [
          /[A-Za-z_][\w]*/,
          {
            cases: {
              "@keywords": "keyword",
              "@builtinFunctions": "keyword",
              "@default": "identifier",
            },
          },
        ],
        // Whitespace
        [/[ \t\r\n]+/, "white"],
      ],
      stringTriple: [
        [/[^"\\]+/, "string"],
        [/"""/, { token: "string.quote", next: "@pop" }],
        [/./, "string"],
      ],
      stringTripleSingle: [
        [/[^'\\]+/, "string"],
        [/'''/, { token: "string.quote", next: "@pop" }],
        [/./, "string"],
      ],
    },
  });

  monaco.languages.setLanguageConfiguration(SPARQL_LANGUAGE_ID, {
    comments: { lineComment: "#" },
    brackets: [
      ["{", "}"],
      ["[", "]"],
      ["(", ")"],
    ],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: "(", close: ")" },
      { open: '"', close: '"', notIn: ["string"] },
      { open: "'", close: "'", notIn: ["string"] },
      { open: "<", close: ">" },
    ],
  });

  monaco.languages.registerCompletionItemProvider(SPARQL_LANGUAGE_ID, {
    triggerCharacters: ["?", "$", ":", "<"],
    provideCompletionItems(model, position) {
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };
      const suggestions: MonacoNS.languages.CompletionItem[] = [
        ...SPARQL_KEYWORDS.map((k) => ({
          label: k,
          kind: monaco.languages.CompletionItemKind.Keyword,
          insertText: k,
          range,
        })),
        ...SPARQL_FUNCTIONS.map((k) => ({
          label: k,
          kind: monaco.languages.CompletionItemKind.Function,
          insertText: k,
          range,
        })),
        ...SPARQL_PREFIXES_DEFAULT.map(({ prefix, iri }) => ({
          label: `PREFIX ${prefix}`,
          kind: monaco.languages.CompletionItemKind.Module,
          insertText: `PREFIX ${prefix}: <${iri}>\n`,
          detail: iri,
          range,
        })),
      ];
      return { suggestions };
    },
  });
}

export { SPARQL_KEYWORDS, SPARQL_PREFIXES_DEFAULT };
