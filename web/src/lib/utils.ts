import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

/**
 * Knows the design system's named text sizes (tokens.css), so `text-meta`
 * and a color like `text-text-muted` aren't mistaken for each other.
 */
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: ["body", "eyebrow", "label", "meta", "metric", "nav", "page", "section", "title"],
    },
  },
});

/** Joins class names, letting later Tailwind utilities override earlier ones. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
