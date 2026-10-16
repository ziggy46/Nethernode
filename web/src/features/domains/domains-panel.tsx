import { useMutation, useQuery } from "@tanstack/react-query";
import { Globe, Plus, X } from "lucide-react";
import { type FormEvent, useId, useState } from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ErrorAlert, Input } from "@/components/ui/field";
import { proxyClient } from "@/features/proxy/client";
import { errorMessage } from "@/lib/api";

import { domainError, domainsQuery, useSetDomains } from "./queries";

/** The domains servers get names under. Administrators. */
export default function DomainsPanel() {
  const domains = useQuery(domainsQuery);
  const save = useSetDomains();
  const inputId = useId();
  const [draft, setDraft] = useState("");
  const [touched, setTouched] = useState(false);
  const list = domains.data ?? [];
  const problem = touched && draft ? domainError(draft) : undefined;

  const add = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    if (!draft.trim() || domainError(draft)) return;
    save.mutate([...list, draft], {
      onSuccess: () => {
        toast.success(`${draft.trim().toLowerCase()} added.`);
        setDraft("");
        setTouched(false);
      },
    });
  };

  return (
    <Panel>
      <PanelHeader title="Domains" icon={<Globe />} />
      <p className="mb-3 text-meta text-text-muted">
        Domains you own, like example.com. Each new server can get a name on one, such as smp.example.com, so
        players join with that name. The first domain is the default.
      </p>
      {domains.isError ? (
        <ErrorAlert>{errorMessage(domains.error)}</ErrorAlert>
      ) : !domains.data ? (
        <div className="h-12 animate-pulse rounded-md bg-surface" role="status" aria-busy />
      ) : list.length === 0 ? (
        <p className="mb-3 rounded-md border border-divider border-dashed px-3 py-2.5 text-meta text-text-muted">
          No domains yet. Without one, servers can still get their own hostname or port when you create them.
        </p>
      ) : (
        <ul className="mb-3 divide-y divide-divider rounded-md border border-divider">
          {list.map((domain, i) => (
            <DomainRow
              key={domain}
              domain={domain}
              first={i === 0}
              busy={save.isPending}
              onRemove={() =>
                save.mutate(
                  list.filter((d) => d !== domain),
                  {
                    onSuccess: () =>
                      toast.success(`${domain} removed. Routes already made with it still work.`),
                  },
                )
              }
            />
          ))}
        </ul>
      )}
      {save.isError && <ErrorAlert>{errorMessage(save.error)}</ErrorAlert>}
      <form onSubmit={add} className="flex gap-2 below-sm:flex-col" noValidate>
        <label htmlFor={inputId} className="sr-only">
          Add a domain
        </label>
        <Input
          id={inputId}
          placeholder="example.com"
          autoCapitalize="none"
          spellCheck={false}
          className="font-mono"
          aria-invalid={problem ? true : undefined}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => setTouched(true)}
        />
        <Button type="submit" disabled={save.isPending || !draft.trim()}>
          <Plus />
          Add domain
        </Button>
      </form>
      {problem && <p className="mt-1.5 text-meta text-danger">{problem}</p>}
    </Panel>
  );
}

function DomainRow({
  domain,
  first,
  busy,
  onRemove,
}: {
  domain: string;
  first: boolean;
  busy: boolean;
  onRemove: () => void;
}) {
  const check = useMutation({ mutationFn: () => proxyClient.checkDns({ hostname: `*.${domain}` }) });
  return (
    <li className="px-3 py-2.5">
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate font-mono text-label">
          {domain}
          {first && <span className="ml-2 font-sans text-meta text-text-muted">default</span>}
        </span>
        <Button size="mini" variant="ghost" disabled={check.isPending} onClick={() => check.mutate()}>
          {check.isPending ? "Checking…" : "Check DNS"}
        </Button>
        <Button
          size="mini"
          variant="ghost"
          aria-label={`Remove ${domain}`}
          disabled={busy}
          onClick={onRemove}
        >
          <X />
        </Button>
      </div>
      <p className="mt-1 text-meta text-text-muted">
        DNS: an <span className="font-mono">A</span> record for{" "}
        <span className="font-mono text-text-secondary">*.{domain}</span> pointing at this machine's public
        address (or your router's).
      </p>
      {(check.isSuccess || check.isError) && (
        <p
          role="status"
          className={
            check.isError || check.data?.error ? "mt-1 text-meta text-warning" : "mt-1 text-meta text-success"
          }
        >
          {check.isError
            ? `Couldn't check: ${errorMessage(check.error)}`
            : check.data.error
              ? `Names under ${domain} don't resolve yet (${check.data.error}).`
              : `Names under ${domain} point to ${check.data.addresses.join(", ")}. Make sure that's this machine.`}
        </p>
      )}
    </li>
  );
}
