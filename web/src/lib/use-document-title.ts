import { useEffect } from "react";

/** Sets the tab title as "<page> · NetherNode". */
export function useDocumentTitle(page: string) {
  useEffect(() => {
    document.title = `${page} · NetherNode`;
  }, [page]);
}
