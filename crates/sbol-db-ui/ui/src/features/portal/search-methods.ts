import type {
  SearchStrategiesResponse,
  SearchStrategyDescriptor,
  StructuredSearchInputKind,
} from "./api";

export type SearchMethod =
  | {
      key: "native";
      kind: "native";
      input: "text";
      label: string;
      description: string;
    }
  | {
      key: "sequence";
      kind: "sequence";
      input: "sequence";
      label: string;
      description: string;
    }
  | {
      key: string;
      kind: "structured";
      input: StructuredSearchInputKind;
      label: string;
      description: string;
      strategy: SearchStrategyDescriptor;
      isDefault: boolean;
    };

const NATIVE_FILTER_PARAMS = [
  "type",
  "role",
  "collection",
  "owner",
  "provenance",
  "created_after",
  "created_before",
  "modified_after",
  "modified_before",
];

export function buildSearchMethods(
  strategies?: SearchStrategiesResponse,
  sequenceEnabled = true
): SearchMethod[] {
  const methods: SearchMethod[] = [
    {
      key: "native",
      kind: "native",
      input: "text",
      label: "Keyword & filters",
      description:
        "Search names, descriptions, identifiers, and biological facets with exact visible counts.",
    },
  ];

  const descriptors = [...(strategies?.items ?? [])].sort((left, right) => {
    const leftDefault = left.id === strategies?.default_strategy ? 0 : 1;
    const rightDefault = right.id === strategies?.default_strategy ? 0 : 1;
    return (
      leftDefault - rightDefault ||
      left.display_name.localeCompare(right.display_name)
    );
  });

  for (const strategy of descriptors) {
    for (const input of strategy.capabilities.inputs) {
      methods.push({
        key: `structured:${strategy.id}:${input}`,
        kind: "structured",
        input,
        label: strategyLabel(strategy, input),
        description: strategy.description,
        strategy,
        isDefault: strategy.id === strategies?.default_strategy,
      });
    }
  }

  if (sequenceEnabled) {
    methods.push({
      key: "sequence",
      kind: "sequence",
      input: "sequence",
      label: "DNA sequence alignment",
      description:
        "Find exact motifs or rank global alignments on either DNA strand.",
    });
  }

  return methods;
}

export function activeSearchMethod(
  params: URLSearchParams,
  methods: SearchMethod[]
): SearchMethod {
  const strategy = params.get("strategy");
  const input = params.get("kind") || "text";
  if (strategy) {
    const structured = methods.find(
      (method) =>
        method.kind === "structured" &&
        method.strategy.id === strategy &&
        method.input === input
    );
    if (structured) return structured;
  }
  if (params.get("kind") === "sequence") {
    const sequence = methods.find((method) => method.kind === "sequence");
    if (sequence) return sequence;
  }
  return methods[0];
}

export function paramsForSearchMethod(
  current: URLSearchParams,
  method: SearchMethod
): URLSearchParams {
  const next = new URLSearchParams(current);
  for (const key of [
    "strategy",
    "kind",
    "mode",
    "cursor",
    "explain",
    "offset",
    "sort",
    "direction",
    "view",
    "limit",
    "compat",
    "compat_warning",
  ]) {
    next.delete(key);
  }

  if (method.kind === "native") return next;

  for (const key of NATIVE_FILTER_PARAMS) {
    if (
      method.kind === "structured" &&
      key === "type" &&
      method.strategy.capabilities.filters.includes("object_type")
    ) {
      continue;
    }
    next.delete(key);
  }

  if (method.kind === "sequence") {
    next.set("kind", "sequence");
    return next;
  }

  next.set("strategy", method.strategy.id);
  next.set("kind", method.input);
  return next;
}

function strategyLabel(
  strategy: SearchStrategyDescriptor,
  input: StructuredSearchInputKind
): string {
  if (strategy.capabilities.inputs.length === 1 && input === "text") {
    return strategy.display_name;
  }
  const inputLabel =
    input === "text"
      ? "text"
      : input === "sequence"
        ? "DNA sequence"
        : "similar object";
  return `${strategy.display_name} · ${inputLabel}`;
}
