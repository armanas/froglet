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

## Settlement setup and recovery

A paid deal is not admitted to execution until its required settlement bundle
has been persisted. Recovery respects another worker's active materialization
claim instead of classifying an in-progress setup as an invariant failure.

If setup is abandoned before admission, recovery cancels the external invoices
it can identify and records a local runtime failure with diagnostic evidence.
It does **not** manufacture a free signed Receipt for a paid Quote: that would
contradict the Quote's settlement bindings. The local terminal status without
a Receipt is an unadmitted setup failure, not verified execution or proof of
settlement. Remote callers must keep that distinction; operators must inspect
the cancellation evidence when external cleanup cannot be confirmed.

## Python sandbox

Python workloads run inside a Linux-native sandbox composed of three kernel
primitives:

- **`landlock` ABI v3** (kernel 6.2+) restricts filesystem access to an explicit
  allow-list, including `truncate`, `ftruncate`, and `O_TRUNC`. Froglet refuses
  Python execution on older Landlock ABIs rather than silently weakening the
  write boundary. The default profile grants read on Python/library paths under
  `/usr/lib`, `/usr/local/lib`, `/lib`, `/lib64`,
  SSL trust stores, `/etc/resolv.conf`, `/etc/hosts`, and `/etc/localtime`,
  and grants write only to the per-invocation tempdir. All other paths are
  denied at the kernel layer — including attempts to `open("/etc/passwd")`
  or write under `/tmp/<other>`.
- **`seccomp`** (via a BPF filter) denies network, process creation, and
  `io_uring` entry points: `socket`, `socketpair`, `connect`, `bind`, `fork`,
  `vfork`, `clone`, `clone3`, and the `io_uring_*` syscalls. Landlock grants
  execute access only to the selected Python interpreter and its ELF loader.
  Everything else is allowed so the
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
Each invocation also has its own process group. Timeout and output-limit
cancellation kills and reaps that group; seccomp keeps the workload to the one
Python process, so its `RLIMIT_AS` and `RLIMIT_CPU` ceilings are aggregate.

**Network access.** Network is denied by default. Network-backed data mounts
(`postgres`, `s3`, `redis`) fail closed until Froglet ships endpoint-scoped
proxying, so data mounts do not re-enable `socket`/`connect`/`bind` today.

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
arbitrary exec. Planned later work includes a container-wrap alternative
sandbox mode and a microVM isolation tier intended for multi-tenant hosted
deployments.
