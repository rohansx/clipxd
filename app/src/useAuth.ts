import { useCallback, useEffect, useState } from "react";
import {
  fetchAuthStatus,
  login as apiLogin,
  logout as apiLogout,
  signup as apiSignup,
  setUsername as apiSetUsername,
  type AuthUser,
} from "./api";

export interface Auth {
  loading: boolean;
  /** false → server runs without auth (local/LAN); the app is open. */
  authEnabled: boolean;
  user: AuthUser | null;
  login: (email: string, password: string) => Promise<void>;
  signup: (email: string, password: string, name?: string, username?: string) => Promise<void>;
  logout: () => Promise<void>;
  /** Claim/update the user's URL slug (e.g. after first GitHub login). */
  setUsername: (username: string) => Promise<AuthUser>;
  refresh: () => void;
}

export function useAuth(): Auth {
  const [loading, setLoading] = useState(true);
  const [authEnabled, setAuthEnabled] = useState(false);
  const [user, setUser] = useState<AuthUser | null>(null);
  const [n, setN] = useState(0);

  useEffect(() => {
    let live = true;
    setLoading(true);
    fetchAuthStatus().then((s) => {
      if (!live) return;
      setAuthEnabled(s.authEnabled);
      setUser(s.user);
      setLoading(false);
    });
    return () => {
      live = false;
    };
  }, [n]);

  // Recording.tsx reads the username straight out of localStorage (so a share link is
  // available the instant a clip is recorded, no API round-trip needed) — keep that mirror
  // in sync with whatever `user` actually is, however it got set (login/signup/claim/logout).
  useEffect(() => {
    try {
      if (user?.username) localStorage.setItem("clipxd:username", user.username);
      else localStorage.removeItem("clipxd:username");
    } catch {
      /* storage may be unavailable */
    }
  }, [user?.username]);

  const login = useCallback(async (email: string, password: string) => {
    setUser(await apiLogin(email, password));
    setAuthEnabled(true);
  }, []);
  const signup = useCallback(async (email: string, password: string, name?: string, username?: string) => {
    setUser(await apiSignup(email, password, name, username));
    setAuthEnabled(true);
  }, []);
  const logout = useCallback(async () => {
    await apiLogout();
    setUser(null);
  }, []);
  const setUsername = useCallback(async (username: string) => {
    const u = await apiSetUsername(username);
    setUser(u);
    return u;
  }, []);
  const refresh = useCallback(() => setN((x) => x + 1), []);

  return { loading, authEnabled, user, login, signup, logout, setUsername, refresh };
}
