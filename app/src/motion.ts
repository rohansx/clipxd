// Tiny, opinionated wrappers around framer-motion so we keep animations
// consistent, performant, and `prefers-reduced-motion` aware across the app.
// Kept small on purpose — the design calls for *subtle* motion (one ease,
// one distance). If you find yourself adding more presets here, push back.

import { useEffect, useState } from "react";
import type { Transition, Variants } from "framer-motion";

/** The single "cinematic" ease used everywhere a surface lifts in. */
export const EASE = [0.34, 1.56, 0.42, 1] as const;
/** Slightly softer ease for fade/mount transitions that should feel light. */
export const EASE_SOFT = [0.22, 1, 0.36, 1] as const;

export const tFadeUp: Transition = { duration: 0.42, ease: EASE_SOFT };
export const tSpring: Transition = { type: "spring", stiffness: 260, damping: 26, mass: 0.7 };

export const vFadeUp: Variants = {
  hidden: { opacity: 0, y: 10 },
  shown: { opacity: 1, y: 0, transition: tFadeUp },
};

export const vStagger = (delay = 0.04, initial = 0): Variants => ({
  hidden: {},
  shown: { transition: { staggerChildren: delay, delayChildren: initial } },
});

/** Mount-only fade up — for non-list elements (sections, cards-on-scroll). */
export const vMount: Variants = {
  hidden: { opacity: 0, y: 12 },
  shown: { opacity: 1, y: 0, transition: tFadeUp },
};

/** track the user's motion preference so we can disable transitions inline
 *  (in addition to the CSS-level media query already in styles.css). */
export function usePrefersReducedMotion(): boolean {
  const [r, setR] = useState(false);
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const apply = () => setR(mq.matches);
    apply();
    mq.addEventListener?.("change", apply);
    return () => mq.removeEventListener?.("change", apply);
  }, []);
  return r;
}
