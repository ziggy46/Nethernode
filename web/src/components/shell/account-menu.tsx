import { cloneElement, lazy, type ReactElement, Suspense, useState } from "react";

const AccountMenuImpl = lazy(() => import("./account-menu-impl"));

type Trigger = ReactElement<{ onClick?: () => void; "aria-haspopup"?: "menu"; "aria-expanded"?: boolean }>;

/**
 * Account actions, opened from the sidebar user row or the top-bar avatar.
 * The menu (Radix + positioning, ~15 KB) loads on first click, keeping it
 * out of every page's first load.
 */
export function AccountMenu({
  children,
  align = "end",
  side = "bottom",
}: {
  children: Trigger;
  align?: "start" | "end";
  side?: "top" | "bottom";
}) {
  const [loaded, setLoaded] = useState(false);
  const [open, setOpen] = useState(false);
  if (!loaded) {
    return cloneElement(children, {
      "aria-haspopup": "menu",
      "aria-expanded": false,
      onClick: () => {
        setLoaded(true);
        setOpen(true);
      },
    });
  }
  return (
    <Suspense fallback={children}>
      <AccountMenuImpl open={open} onOpenChange={setOpen} align={align} side={side}>
        {children}
      </AccountMenuImpl>
    </Suspense>
  );
}
