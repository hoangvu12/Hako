import { useState } from "react";
import { CheckCircle, Trash, XCircle } from "@phosphor-icons/react";

import { Spinner } from "@/components/ui/spinner";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { useTestProvider } from "@/hooks/use-cloud";
import type { ProviderConfig } from "@/lib/api";
import { describe } from "./config";

type TestResult = { ok: boolean; message: string } | "pending" | null;

/**
 * One provider list row. Owns its own connectivity-test state + mutation, so a
 * "Test" on one provider re-renders only that row — not the whole list.
 */
export function ProviderRow({
  provider,
  onRemove,
}: {
  provider: ProviderConfig;
  onRemove: (id: string) => void;
}) {
  const test = useTestProvider();
  const [result, setResult] = useState<TestResult>(null);

  const runTest = () => {
    setResult("pending");
    test.mutate(provider.id, {
      onSuccess: () => setResult({ ok: true, message: "Connected" }),
      onError: (e) => setResult({ ok: false, message: String(e) }),
    });
  };

  return (
    <li className="flex items-center gap-3 rounded-lg border border-border/70 bg-card/40 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{provider.label}</div>
        <div className="truncate text-xs text-muted-foreground">{describe(provider.kind)}</div>
      </div>
      {result && result !== "pending" ? (
        <span
          className={cn(
            "flex items-center gap-1 text-xs",
            result.ok ? "text-success" : "text-destructive",
          )}
          title={result.message}
        >
          {result.ok ? (
            <CheckCircle weight="fill" className="size-4" />
          ) : (
            <XCircle weight="fill" className="size-4" />
          )}
        </span>
      ) : null}
      <Button variant="secondary" size="sm" disabled={result === "pending"} onClick={runTest}>
        {result === "pending" ? <Spinner className="size-3.5" /> : null}
        Test
      </Button>
      <Button
        variant="ghost"
        size="icon"
        aria-label={`Remove ${provider.label}`}
        onClick={() => onRemove(provider.id)}
      >
        <Trash className="size-4" />
      </Button>
    </li>
  );
}
