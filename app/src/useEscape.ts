import { useEffect, useRef } from "react";

/** Close-on-Escape for overlays. The callback lives in a ref so inline arrow props
 *  (`onClose={() => setX(null)}`) don't tear down and re-subscribe the listener on
 *  every render. */
export function useEscape(onClose: () => void): void {
  const cb = useRef(onClose);
  cb.current = onClose;
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") cb.current();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);
}
