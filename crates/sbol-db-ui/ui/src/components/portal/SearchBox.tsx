import { FormEvent, useEffect, useState } from "react";
import { Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

interface SearchBoxProps {
  initialQuery?: string;
  placeholder?: string;
  onSearch: (query: string) => void;
  autoFocus?: boolean;
  className?: string;
  size?: "default" | "hero";
}

export function SearchBox({
  initialQuery = "",
  placeholder = "Search by name, description, type, or identifier…",
  onSearch,
  autoFocus,
  className,
  size = "default",
}: SearchBoxProps) {
  const [query, setQuery] = useState(initialQuery);

  useEffect(() => setQuery(initialQuery), [initialQuery]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSearch(query.trim());
  };

  return (
    <form
      role="search"
      onSubmit={submit}
      className={cn(
        "flex items-center rounded-xl border bg-background p-1.5 shadow-sm focus-within:border-primary/50 focus-within:ring-4 focus-within:ring-primary/10",
        size === "hero" && "rounded-2xl p-2 shadow-lg shadow-primary/5",
        className
      )}
    >
      <Search
        aria-hidden="true"
        className={cn(
          "ml-2.5 size-4 shrink-0 text-muted-foreground",
          size === "hero" && "ml-3 size-5"
        )}
      />
      <Input
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder={placeholder}
        autoFocus={autoFocus}
        aria-label="Search the registry"
        className={cn(
          "h-9 border-0 bg-transparent shadow-none focus-visible:ring-0",
          size === "hero" && "h-11 px-4 text-base"
        )}
      />
      <Button type="submit" size={size === "hero" ? "lg" : "default"}>
        Search
      </Button>
    </form>
  );
}
