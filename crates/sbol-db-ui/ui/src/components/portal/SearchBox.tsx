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
        "flex items-center border border-input bg-card shadow-[0_2px_0_hsl(var(--foreground)/0.05)] transition-[border-color,box-shadow] duration-150 focus-within:border-primary/65 focus-within:shadow-[0_0_0_3px_hsl(var(--primary)/0.10)]",
        size === "hero" && "border-foreground/25",
        className
      )}
    >
      <Search
        aria-hidden="true"
        className={cn(
          "ml-3 size-4 shrink-0 text-muted-foreground",
          size === "hero" && "ml-4 size-5"
        )}
      />
      <Input
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder={placeholder}
        autoFocus={autoFocus}
        aria-label="Search the registry"
        className={cn(
          "h-10 border-0 bg-transparent shadow-none focus-visible:ring-0",
          size === "hero" && "h-14 px-4 text-base"
        )}
      />
      <Button
        type="submit"
        size={size === "hero" ? "lg" : "default"}
        className={cn(
          "self-stretch rounded-none border-l border-primary/30",
          size === "hero" && "h-auto px-6 sm:px-8"
        )}
      >
        Search
      </Button>
    </form>
  );
}
