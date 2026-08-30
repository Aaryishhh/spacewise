import { createContext, useCallback, useContext, useState, ReactNode } from "react";
import { CleanupCandidate } from "../api";

// "Add to Cleanup" from the treemap (or anywhere else) must never delete
// directly -- it only ever adds a candidate here, and the Cleanup page is
// the single place that builds a plan, gets user approval, and calls
// execute_cleanup. This context is that one shared basket.
interface ManualBasketValue {
  items: CleanupCandidate[];
  add: (item: Omit<CleanupCandidate, "id">) => void;
  remove: (id: string) => void;
  clear: () => void;
}

const ManualBasketContext = createContext<ManualBasketValue | null>(null);

export function ManualBasketProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<CleanupCandidate[]>([]);

  const add = useCallback((item: Omit<CleanupCandidate, "id">) => {
    setItems((prev) => {
      if (prev.some((p) => p.paths[0] === item.paths[0])) return prev;
      return [...prev, { ...item, id: crypto.randomUUID() }];
    });
  }, []);

  const remove = useCallback((id: string) => {
    setItems((prev) => prev.filter((i) => i.id !== id));
  }, []);

  const clear = useCallback(() => setItems([]), []);

  return <ManualBasketContext.Provider value={{ items, add, remove, clear }}>{children}</ManualBasketContext.Provider>;
}

export function useManualBasket(): ManualBasketValue {
  const ctx = useContext(ManualBasketContext);
  if (!ctx) throw new Error("useManualBasket must be used within a ManualBasketProvider");
  return ctx;
}
