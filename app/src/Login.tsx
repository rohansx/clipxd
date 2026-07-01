import { useState } from "react";
import { Brand } from "./Brand";
import { githubLoginUrl } from "./api";

interface LoginProps {
  onLogin: (email: string, password: string) => Promise<void>;
  onSignup: (
    email: string,
    password: string,
    name?: string,
    username?: string
  ) => Promise<void>;
}

const SLUG_RE = /^[a-z0-9_-]{3,30}$/;

/**
 * Auth form contents only — the .auth-screen / .auth-card wrapper lives in
 * App.tsx, where it can also host the "← landing" back button.
 */
export function Login({ onLogin, onSignup }: LoginProps) {
  const [mode, setMode] = useState<"login" | "signup">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [username, setUsername] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const submit = async () => {
    setErr(null);
    if (!email.includes("@")) return setErr("Enter a valid email.");
    if (password.length < 8) return setErr("Password must be at least 8 characters.");
    setBusy(true);
    try {
      if (mode === "signup") {
        const trimmedUser = username.trim();
        if (trimmedUser && !SLUG_RE.test(trimmedUser)) {
          setBusy(false);
          return setErr("Username must be 3-30 chars (lowercase letters, digits, '-' or '_').");
        }
        await onSignup(email.trim(), password, name.trim() || undefined, trimmedUser || undefined);
      } else {
        await onLogin(email.trim(), password);
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Something went wrong.");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <div className="auth-head">
        <Brand size={36} withWord />
        <p className="auth-tag">Record once. Humans watch it. Agents read it.</p>
      </div>

      <a className="btn auth-github" href={githubLoginUrl()}>
        <svg width="17" height="17" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
          <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z" />
        </svg>
        Continue with GitHub
      </a>

      <div className="auth-or"><span>or</span></div>

      {mode === "signup" && (
        <>
          <input className="input" placeholder="Name (optional)" value={name} onChange={(e) => setName(e.target.value)} />
          <input
            className="input"
            placeholder="username (your share-link slug, optional)"
            value={username}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setUsername(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ""))}
          />
        </>
      )}
      <input
        className="input"
        type="email"
        placeholder="you@example.com"
        value={email}
        autoComplete="email"
        onChange={(e) => setEmail(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && submit()}
      />
      <input
        className="input"
        type="password"
        placeholder={mode === "signup" ? "Choose a password (8+ chars)" : "Password"}
        value={password}
        autoComplete={mode === "signup" ? "new-password" : "current-password"}
        onChange={(e) => setPassword(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && submit()}
      />

      {err && <div className="auth-err">{err}</div>}

      <button className="btn-signal btn-pill" onClick={submit} disabled={busy} style={{ width: "100%", height: 42 }}>
        {busy ? <span className="spin" /> : mode === "signup" ? "Create account" : "Sign in"}
      </button>

      <div className="auth-switch">
        {mode === "login" ? (
          <>
            New here?{" "}
            <button className="auth-link" onClick={() => { setMode("signup"); setErr(null); }}>
              Create an account
            </button>
          </>
        ) : (
          <>
            Already have an account?{" "}
            <button className="auth-link" onClick={() => { setMode("login"); setErr(null); }}>
              Sign in
            </button>
          </>
        )}
      </div>
    </>
  );
}

