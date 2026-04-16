# Runtime

`froglet-runtime` remains the deal and payment engine used when a Froglet node
invokes remote Froglet resources.

It still owns:

- remote node resolution
- quote fetch and verification
- local deal signing
- remote deal submission
- local deal state
- payment intent exposure
- result acceptance

A single Froglet node may both publish local resources and invoke remote ones.
`provider` and `requester` remain per-deal roles, not node classes.

What changed in this cutover is the bot-facing shape above it:

- bots no longer talk to many role-specific plugin tools
- bots talk to one local control surface through one tool: `froglet`

Named services, data services, and open-ended compute all compile down to the
same underlying Froglet deal flow.

At the product surface:

- named and data-service bindings are discovered and invoked through service
  metadata
- open-ended compute uses the provider's direct compute offer
- bounded async execution is exposed through task polling
- longer-running orchestration, batch workflows, and checkpoint/resume remain
  higher-layer concerns above the runtime

Current implementation note:

- the checked-in execution profiles are current reference implementations
- the intended product boundary is a generic execution primitive that can back
  named services, data services, and open-ended compute

## Python sandbox

Python workloads run inside a Linux-native sandbox composed of three kernel
primitives:

- **`landlock`** (kernel 5.13+) restricts filesystem access to an explicit
  allow-list. The default profile grants read on `/usr`, `/lib`, `/lib64`,
  SSL trust stores, `/etc/resolv.conf`, `/etc/hosts`, and `/etc/localtime`,
  and grants write only to the per-invocation tempdir. All other paths are
  denied at the kernel layer — including attempts to `open("/etc/passwd")`
  or write under `/tmp/<other>`.
- **`seccomp`** (via a BPF filter) denies a targeted set of syscalls that
  form the practical escape surface: `execve`, `execveat`, `socket`,
  `socketpair`, `connect`, and `bind`. Everything else is allowed so the
  Python stdlib runs unmodified. This is a **deny-list** rather than a
  syscall-level allow-list — a smaller, more auditable policy that closes
  the concrete threats (arbitrary exec and outbound network) without
  enumerating every syscall Python may need.
- **`prctl(PR_SET_NO_NEW_PRIVS, 1)`** neutralises suid binaries and is a
  precondition for installing seccomp without `CAP_SYS_ADMIN`, which means
  the sandbox composes cleanly inside an unprivileged Docker container.

The sandbox lives in [`src/python_sandbox.rs`](../src/python_sandbox.rs) and
is applied via `Command::pre_exec` in the fork-before-exec window, so the
restrictions inherit to the `python3` process across the `execve` boundary.

**Network access.** Network is denied by default. A workload that has been
granted a mount kind requiring network (for example, a future `postgres`
mount) may set `SandboxConfig::allow_network = true`, which re-enables the
`socket`/`connect`/`bind` syscalls for that invocation.

**Non-Linux hosts.** The sandbox is Linux-only. On macOS / other dev hosts
Python execution is refused by default; operators can set
`FROGLET_ALLOW_UNSANDBOXED_PYTHON=1` to opt back in for local development.
Production deploys are Linux (Docker or host) and run sandboxed without
extra configuration.

**Why not namespaces?** `unshare(CLONE_NEWUSER|CLONE_NEWPID|…)` was
considered but omitted as the default: Docker's seccomp profile denies
nested `unshare` without `--privileged` or a relaxed profile, and
Froglet's primary deploy is Docker Compose. Landlock and seccomp both
work unchanged nested inside Docker, and together close the same threat
surface namespaces would — filesystem exfiltration, outbound network, and
arbitrary exec. See
[TODO.md Order 73](../TODO.md) for the planned container-wrap alternative
sandbox mode and [Order 75](../TODO.md) for the microVM isolation tier
intended for multi-tenant hosted deployments.
