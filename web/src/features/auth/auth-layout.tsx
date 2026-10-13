import type * as React from "react";

import { Logo, Wordmark } from "@/components/brand";

/**
 * Centered card on the page background with the icon above it and a 2px
 * crimson top edge, the one decorative crimson moment (06 §6).
 */
export function AuthLayout({
  title,
  description,
  children,
  footer,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center px-4 py-12">
      <div className="mb-7 flex flex-col items-center gap-3">
        <Logo className="size-20" />
        <Wordmark className="text-[22px]" />
      </div>
      <section
        aria-labelledby="auth-title"
        className="w-full max-w-[400px] rounded-xl border border-border border-t-2 border-t-crimson bg-surface p-7 below-sm:p-5"
      >
        <h1 id="auth-title" className="mb-1.5 font-semibold text-title">
          {title}
        </h1>
        <p className="mb-6 text-label text-text-muted">{description}</p>
        {children}
      </section>
      {footer && <div className="mt-6 text-label text-text-muted">{footer}</div>}
    </main>
  );
}
