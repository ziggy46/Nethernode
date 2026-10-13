import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowRight, Box, Cpu, History, MemoryStick, Plus, Server, ShieldCheck, Users } from "lucide-react";

import {
  EmptyState,
  Footer,
  MetricCard,
  PageHeader,
  Panel,
  PanelHeader,
  SectionHeading,
} from "@/components/page";
import { Button } from "@/components/ui/button";
import { healthQuery } from "@/lib/health";
import { useDocumentTitle } from "@/lib/use-document-title";

/*
 * Overview, laid out after the UI reference. Server management, host
 * metrics, and backups don't exist yet, so every panel shows what is
 * actually true (no servers, nothing measured) rather than sample data.
 */

const SERVER_CREATION_UNAVAILABLE = "Server creation isn't available in this version yet.";

export function OverviewPage() {
  useDocumentTitle("Overview");
  const health = useQuery(healthQuery);

  return (
    <>
      <PageHeader
        eyebrow="Your control center"
        title="Your worlds, under control."
        description="A good day to build something. Here's how your servers are doing."
        actions={
          <>
            <Button asChild variant="subtle" className="below-md:hidden">
              <Link to="/backups">
                <History />
                Backups
              </Link>
            </Button>
            <Button variant="primary" disabled title={SERVER_CREATION_UNAVAILABLE}>
              <Plus />
              Create server
            </Button>
          </>
        }
      />

      <div className="mb-[30px] grid grid-cols-4 gap-3.5 below-xl:gap-2.5 below-md:mb-6 below-md:grid-cols-2">
        <MetricCard
          label="Active servers"
          icon={<Server />}
          value="0"
          unit="/ 0 servers"
          note="No servers yet"
        />
        <MetricCard
          label="Players online"
          icon={<Users />}
          value="0"
          unit="across your worlds"
          note="No servers to join yet"
        />
        <MetricCard
          label="Memory usage"
          icon={<MemoryStick />}
          value="—"
          note="Host metrics aren't connected yet"
        />
        <MetricCard label="Latest backup" icon={<ShieldCheck />} value="None" note="No backups yet" />
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_var(--nn-right-rail-width)] gap-6 below-xl:gap-[18px] below-lg:grid-cols-1 below-md:gap-[22px]">
        <section aria-labelledby="servers-heading">
          <div id="servers-heading">
            <SectionHeading title="Your servers" count={0} caption="One workspace. Endless worlds." />
          </div>
          <div className="rounded-lg border border-divider border-dashed bg-surface-subtle">
            <EmptyState icon={<Server />} title="No servers yet">
              Your Minecraft servers will appear here. {SERVER_CREATION_UNAVAILABLE}
            </EmptyState>
          </div>
        </section>

        <aside
          aria-label="Workspace details"
          className="flex flex-col gap-[18px] below-lg:grid below-lg:grid-cols-2 below-lg:gap-[15px] below-sm:grid-cols-1"
        >
          <Panel>
            <PanelHeader title="Host resources" icon={<Cpu />} />
            <EmptyState compact icon={<Cpu />} title="Not measured yet">
              CPU, memory, and storage for this host will appear here once host monitoring is available.
            </EmptyState>
            <div className="mt-[13px] flex justify-between border-divider border-t pt-[13px] text-meta text-text-muted">
              <span>Panel</span>
              <span className="text-text-secondary">
                {health.isSuccess
                  ? `Online · v${health.data.version}`
                  : health.isError
                    ? "Unreachable"
                    : "Checking…"}
              </span>
            </div>
          </Panel>

          <Panel>
            <PanelHeader title="Recent activity" icon={<History />} />
            <EmptyState compact icon={<History />} title="Nothing to show yet">
              Server, player, and backup activity will appear here.
            </EmptyState>
          </Panel>

          <section className="rounded-lg border border-primary/35 border-dashed bg-primary-subtle p-4 below-lg:col-span-full below-lg:flex below-lg:items-center below-lg:gap-5 below-sm:block">
            <h3 className="mb-1.5 flex items-center gap-[7px] font-[550] text-label">
              <Box className="size-[15px] text-primary" aria-hidden />
              Your next adventure
            </h3>
            <p className="mb-3 text-meta text-text-muted below-lg:m-0 below-lg:flex-1 below-sm:my-2">
              Explore modpacks and spin up a new world in a few clicks.
            </p>
            <Button asChild variant="accent" size="mini" className="w-full below-lg:w-auto below-sm:w-full">
              <Link to="/modpacks">
                Browse modpacks
                <ArrowRight />
              </Link>
            </Button>
          </section>
        </aside>
      </div>

      <Footer version={health.data?.version} />
    </>
  );
}
