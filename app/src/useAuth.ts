import { useCallback, useEffect, useState } from "react";
import { fetchAuthStatus, login as apiLogin, logout as apiLogout, signup as apiSignup, type AuthUser } from "./api";

export interface Auth {
  loading: boolean;
  /** false → server runs without auth (local/LAN); the app is open. */
  authEnabled: boolean;
  user: AuthUser | null;
  login: (email: string, password: string) => Promise<void>;
  signup: (email: string, password: string, name?: string) => Promise<void>;
  logout: () => Promise<void>;
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

  const login = useCallback(async (email: string, password: string) => {
    setUser(await apiLogin(email, password));
    setAuthEnabled(true);
  }, []);
  const signup = useCallback(async (email: string, password: string, name?: string) => {
    setUser(await apiSignup(email, password, name));
    setAuthEnabled(true);
  }, []);
  const logout = useCallback(async () => {
    await apiLogout();
    setUser(null);
  }, []);
  const refresh = useCallback(() => setN((x) => x + 1), []);

  return { loading, authEnabled, user, login, signup, logout, refresh };
}
