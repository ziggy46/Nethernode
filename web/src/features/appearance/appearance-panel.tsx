import { useMutation } from "@tanstack/react-query";
import { Monitor, Moon, Settings, Sun } from "lucide-react";
import type * as React from "react";
import { toast } from "sonner";

import { Panel, PanelHeader } from "@/components/page";
import { accountClient } from "@/features/account/api";
import { errorMessage } from "@/lib/api";
import { setThemeChoice, type ThemeChoice, useThemeChoice } from "@/lib/theme";
import { cn } from "@/lib/utils";

import { THEME_TO_PROTO } from "./sync";

const CHOICES: { value: ThemeChoice; label: string; icon: React.ReactNode }[] = [
  { value: "system", label: "System", icon: <Monitor /> },
  { value: "dark", label: "Dark", icon: <Moon /> },
  { value: "light", label: "Light", icon: <Sun /> },
];

/** Preferences that follow the account: for now, the color theme. */
export default function AppearancePanel() {
  const choice = useThemeChoice();
  const save = useMutation({
    mutationFn: (value: ThemeChoice) =>
      accountClient.updatePreferences({ preferences: { theme: THEME_TO_PROTO[value] } }),
    onError: (e) => toast.error(`Couldn't save the theme to your account: ${errorMessage(e)}`),
  });
  const choose = (value: ThemeChoice) => {
    setThemeChoice(value);
    save.mutate(value);
  };

  return (
    <Panel>
      <PanelHeader title="General preferences" icon={<Settings />} />
      <div className="flex items-center justify-between gap-5 below-sm:flex-col below-sm:items-start">
        <div className="min-w-0">
          <p className="font-[550] text-label" id="appearance-label">
            Appearance
          </p>
          <p className="mt-[3px] text-meta text-text-muted">
            Follow your device, or always use dark or light. Saved to your account.
          </p>
        </div>
        <div
          role="radiogroup"
          aria-labelledby="appearance-label"
          className="flex shrink-0 rounded-md border border-border-strong bg-field p-0.5"
        >
          {CHOICES.map((c) => (
            <label
              key={c.value}
              className={cn(
                "flex cursor-pointer items-center gap-1.5 rounded-[5px] px-2.5 py-1 text-label transition-colors duration-(--nn-transition) [&_svg]:size-3.5",
                "has-focus-visible:outline-2 has-focus-visible:outline-primary",
                choice === c.value
                  ? "bg-nav-active text-nav-active-foreground"
                  : "text-text-secondary hover:bg-button-quiet-hover hover:text-text-primary",
              )}
            >
              <input
                type="radio"
                name="theme"
                value={c.value}
                checked={choice === c.value}
                onChange={() => choose(c.value)}
                className="sr-only"
              />
              {c.icon}
              {c.label}
            </label>
          ))}
        </div>
      </div>
    </Panel>
  );
}
