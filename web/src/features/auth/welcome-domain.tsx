import { useNavigate } from "@tanstack/react-router";
import { ArrowRight } from "lucide-react";
import { type FormEvent, useState } from "react";

import { Button } from "@/components/ui/button";
import { ErrorAlert, Field } from "@/components/ui/field";
import { errorMessage } from "@/lib/api";

import { domainError, useSetDomains } from "../domains/queries";

/** First-run step after the owner account: the domain servers go under. */
export function WelcomeDomain() {
  const navigate = useNavigate();
  const save = useSetDomains();
  const [domain, setDomain] = useState("");
  const [touched, setTouched] = useState(false);
  const problem = touched ? domainError(domain) : undefined;
  const shown = domain.trim().toLowerCase().replace(/^\*\./, "").replace(/\.$/, "") || "example.com";

  const finish = () => void navigate({ to: "/" });
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (domainError(domain)) return;
    save.mutate([domain], { onSuccess: finish });
  };

  return (
    <form onSubmit={onSubmit} noValidate>
      {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
      <Field
        label="Your domain"
        placeholder="example.com"
        autoCapitalize="none"
        spellCheck={false}
        autoFocus
        inputClassName="font-mono"
        hint="A domain you own. You can add more, or change it, in Settings."
        error={problem}
        value={domain}
        onChange={(e) => setDomain(e.target.value)}
      />
      <div className="mb-[17px] rounded-md border border-divider bg-surface-subtle px-3 py-2.5 text-meta text-text-muted">
        <p>
          Players will join servers at names like{" "}
          <span className="font-mono text-text-secondary">smp.{shown}</span>.
        </p>
        <p className="mt-1">
          In your DNS, add an <span className="font-mono">A</span> record for{" "}
          <span className="font-mono text-text-secondary">*.{shown}</span> pointing at this machine's public
          address (or your router's).
        </p>
      </div>
      <Button type="submit" variant="primary" size="lg" className="w-full" disabled={save.isPending}>
        {save.isPending ? "Saving…" : "Save and continue"}
        <ArrowRight />
      </Button>
      <button
        type="button"
        className="mt-4 w-full text-center text-label text-text-muted underline-offset-2 hover:text-text-primary hover:underline"
        onClick={finish}
      >
        Skip for now
      </button>
    </form>
  );
}
