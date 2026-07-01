# CI / CD — clipxd

Both workflows live under `.github/workflows/`.

## `ci.yml` — runs on every PR + direct push (master, design/**)

Two parallel jobs, both required:

| Job       | What it checks                                                   | Time   |
| --------- | ---------------------------------------------------------------- | ------ |
| `web`     | `tsc --noEmit` + `vite build`, plus a smoke check that the SEO assets actually ship (og-image.svg, JSON-LD, manifest, etc.) | ~1 min |
| `rust`    | `cargo check --workspace --all-targets` (no link, no build — just the types) | ~3 min |

Concurrency-cancelled on the same ref, so a `git push` + `git push --force` doesn't double up.

## `deploy.yml` — runs on every push to **master** + manual `workflow_dispatch`

1. SSH from the runner into `clipxd@clipxd.com` using the `clipxd-ci` key
   (NOT the admin `clipxd_deploy` key).
2. That key is locked down in `/opt/clipxd/.ssh/authorized_keys` to a
   **forced command**: only `/home/clipxd/.github-actions/ci-deploy.sh`
   can run. The wrapper does:
   - `git fetch github master`
   - bail out if local HEAD ≠ origin/master (no racing another in-flight deploy)
   - run the existing `./deploy/deploy.sh` (which builds the SPA + Rust binaries, syncs the assets, restarts the systemd unit, reloads Caddy)
   - log to `/var/log/clipxd-ci-deploy.log`
3. The runner then curls the deployed site to confirm the new SPA is live.

## One-time setup (the user has to do exactly **once**)

After merging this PR, in **github.com/rohansx/clipxd → Settings →
Secrets and variables → Actions → New repository secret**:

| Secret name    | Value                                       |
| -------------- | ------------------------------------------- |
| `BOX_SSH_HOST` | `clipxd.com`                                |
| `BOX_SSH_KEY`  | the entire `clipxd-ci` private key         |

To grab the private key:
```bash
sudo -u clipxd bash /home/clipxd/clipxd/tools/show-ci-secret.sh
```

That prints the same `-----BEGIN OPENSSH PRIVATE KEY-----` block you
paste into GitHub. The key is **restricted** in
`/opt/clipxd/.ssh/authorized_keys` to a forced command
(`/home/clipxd/.github-actions/ci-deploy.sh`) — so even if it leaks,
it cannot get a shell, cannot read files, cannot port-forward. Worst
case it just runs the deploy script.

## Diagram

```
                   (1) `push` event
                                │
                                ▼
                    ┌──────────────────────┐
                    │  ci.yml  (1-3 min)   │
                    │   web  + rust jobs   │
                    └──────────┬───────────┘
                               │ required status check
                               ▼
                    ┌──────────────────────┐
                    │  deploy.yml  (~5 min)│
                    └──────────┬───────────┘
                               │
              appleboy/ssh-action
              key=BOX_SSH_KEY   host=clipxd.com
                               │
                               ▼
       ┌─────────────────────────────────────────┐
       │  /opt/clipxd/.ssh/authorized_keys       │
       │  └─ forced-command: ci-deploy.sh         │
       └──────────┬──────────────────────────────┘
                  │
                  ▼
      /home/clipxd/.github-actions/ci-deploy.sh
        ├─ git fetch github master
        ├─ if HEAD stale → exit 2 (no deploy)
        ├─ ./deploy/deploy.sh                  ── on box
        │   ├─ tsc --noEmit + vite build
        │   ├─ cargo build --release (static musl)
        │   ├─ systemctl restart clipxd-web
        │   └─ systemctl reload caddy
        └─ log to /var/log/clipxd-ci-deploy.log
```

## Manual redeploy

`Actions → deploy → Run workflow` with an optional ref. Useful when a
push-to-master hook didn't fire (e.g. secret expired and you rotated it).
