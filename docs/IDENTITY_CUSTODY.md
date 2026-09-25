# Identity custody and continuity

Froglet has two independent long-lived secp256k1 identities:

- the node identity, which is the provider identity used for Froglet signatures;
- the Nostr publication identity, which signs publication events.

Both must survive host loss. The custody layer is outside the Kernel: it does
not change artifact bytes, hashing, signature domains, or state transitions.

## Default clean-host flow

The default requires no cloud account, password KDF, shell tool, or OS
keychain. Froglet generates a random 256-bit recovery-key file and encrypts
both seeds in one authenticated `AES-256-GCM-SIV` bundle:

```sh
froglet-node identity generate --json
froglet-node identity backup \
  --output "$HOME/froglet-identity-backup.json" \
  --recovery-key "$HOME/froglet-identity.recovery-key" \
  --json
```

The two output files are created atomically with mode `0600` and are never
overwritten. Froglet prints their paths, public identities, and the backup
digest; it never prints seed bytes or recovery-key bytes. Store the encrypted
backup and recovery key in separate secure locations. Keeping both only on the
node does not provide disaster recovery.

Before writing the bundle or marking it current, Froglet asks the configured
adapter to unseal its own output and verifies the exact seed payload and both
derived public identities. An opaque-looking but unrecoverable adapter result,
or an adapter that returns the plaintext as its sealed output, fails without
creating a backup marker.

Restore is fail-closed and only targets a fresh identity directory:

```sh
FROGLET_DATA_DIR="$HOME/restored-froglet" \
  froglet-node identity restore \
    --backup /secure/location/froglet-identity-backup.json \
    --recovery-key /separate/location/froglet-identity.recovery-key \
    --json
```

The bundle's public node and Nostr identities are authenticated as associated
data and are checked again after decryption. A wrong key, modified bundle, bad
seed, existing target identity directory, symlinked recovery key, or
group/world-readable recovery key fails before any identity file is installed.
Restore stages both `0600` seed files in a private directory and atomically
renames that directory into place. Before installing the status or identity,
both restored keys sign a private mode-`0600` restore journal that binds the
exact destination, staged directory, public identities, and backup-status
record. The journal and staging directory are synced first, then the status is
synced, then the identity directory is activated and the data directory is
synced. Only then is the journal removed and that removal synced.

Daemon startup and every identity CLI path recover this journal before any key
can be loaded or auto-generated. A durable journal deterministically rolls the
authenticated restore forward; unjournaled staging is rolled back. Modified
journal claims, missing staged secrets, ambiguous installed/staged directories,
or a blocked status path fail closed without generating a replacement key.

`froglet-node identity status --json` verifies that the current identities
match the last backup marker, that both status and bundle are private regular
files rather than symlinks, and that the referenced canonical v1 bundle still
exists with its exact SHA-256 digest. It parses the bundle and binds its
creation time, both public identities, protection kind, recovery nonce or
operator descriptor, and bounded non-empty ciphertext to the marker. A
well-digested malformed or mismatched file is therefore not `current`. This is
non-destructive inspection, not authorization to rotate: rotation must also
prove live recovery with the configured custody adapter.

Public publication preflight copies only the resulting structured state and
bundle digest into its non-Kernel approval precondition; local paths and
custody identifiers remain private. The provider recomputes that state
immediately before publication mutation. A changed, missing, stale, or
corrupted backup invalidates the old approval and is disclosed in the new
plan; private local proof explicitly does not require a backup.

## Rotation and independent continuity proof

Create and verify a current backup before rotation. Stop the daemon first so
the running process cannot continue advertising the old in-memory identity.

```sh
froglet-node identity rotate \
  --recovery-key /separate/location/froglet-identity.recovery-key \
  --reason "scheduled operator rotation" \
  --json

froglet-node identity verify-continuity \
  --record /path/from/the/rotation/report.json \
  --json
```

The continuity payload binds the old node identity, new node identity, stable
Nostr publication identity, timestamp, and operator reason. It carries three
BIP340 Schnorr signatures: one by the old node key, one by the new node key,
and one by the Nostr publication key. Verification needs only the public JSON
record. Rotation never renames the canonical seed out of place. It first
persists a mode-`0600` next-generation seed and a durable hard link to the
previous seed, then writes a private rotation journal containing the signed
continuity record. The public continuity record is installed and its directory
synced before one atomic rename activates the next seed. The identity directory
is synced before the previous generation and journal are removed. Thus the
canonical path always resolves to the old or new complete seed, and the new
identity cannot become durable without its continuity proof already durable.

If the process or host stops at any boundary, daemon startup and identity CLI
startup recover under an advisory OS lock before loading the node identity;
`identity generate`, `backup`, and rerunning `identity rotate` perform the same
recovery before considering a new key or backup. Restore and rotation share the
same data-directory custody lock, so another process cannot observe their
intermediate files. A durable rotation journal rolls forward after validating the old, new, and
stable Nostr public identities plus all three signatures. Pre-journal staging
is rolled back. If an un-synced activation is observed with neither the new
canonical entry nor its next-generation source, the preserved old hard link is
restored and the owned continuity record is removed. The lock is
descriptor-backed, so process exit releases it even though its private inode
remains. Rotation then marks the previous backup stale and requires a new
backup and daemon restart.

After startup recovery and identity load, Froglet also reconciles publication
truth: any active lifecycle whose verified signed Publication Revision names a
different `provider_id` is atomically paused. Immutable revisions and the
selected revision are preserved for audit, but the old identity's revision is
not advertised or resumable as active; the service must be republished under
the current provider identity.

Under the rotation lock, immediately before replacing the node seed, Froglet
loads the bundle referenced by the current status record, checks its digest and
metadata, unseals it with the supplied recovery key or operator command, and
constant-time compares the recovered seed payload with the current identities.
A stale digest-only marker or wrong custody credential cannot authorize the
destructive step. Operator-command custody uses the same credential flags as
restore:

```sh
froglet-node identity rotate \
  --operator-command /opt/froglet/bin/custody-wrapper \
  --key-id projects/example/locations/global/keyRings/froglet/cryptoKeys/node \
  --reason "scheduled operator rotation" \
  --json
```

Only the node/provider identity rotates in v1. The Nostr publication identity
is deliberately stable and co-signs the continuity record; independent Nostr
rotation needs a publication migration protocol and is not silently conflated
with provider rotation.

## KMS and HSM adapter

Operators can supply an absolute-path executable implementing
`froglet-custody-command-v1`:

```sh
froglet-node identity backup \
  --output /secure/froglet-identity-backup.json \
  --operator-command /opt/froglet/bin/custody-wrapper \
  --key-id projects/example/locations/global/keyRings/froglet/cryptoKeys/node \
  --json
```

Froglet invokes the executable directly, never through a shell:

- `PROGRAM seal --key-id ID` receives the fixed-length secret payload on
  stdin and must return an authenticated opaque blob on stdout;
- `PROGRAM unseal --key-id ID` receives that blob and must return the exact
  secret payload on stdout;
- public authenticated metadata is available as base64 in
  `FROGLET_CUSTODY_AAD_BASE64`;
- stdin and stdout are serviced concurrently, so wrappers may stream in both
  directions without filling one pipe while Froglet blocks on the other;
- each invocation has a hard 30-second deadline; timeout or an oversized output
  kills and reaps the wrapper's process group;
- the inherited environment is cleared and rebuilt from a small operational
  allowlist (`HOME`, executable/search paths, temp/locale/TLS paths, and HTTP
  proxy settings) plus the authenticated-data variable. In particular, Froglet
  identity-seed variables and unrelated daemon secrets are not inherited;
- stderr is discarded, output is capped, and decrypted output is zeroized.

The wrapper owns cloud authentication and must bind the supplied associated
data in its authenticated encryption. It should use workload identity, metadata
credentials, or configuration reachable through the allowlisted `HOME`; an
arbitrary parent-process secret environment is intentionally unavailable. This
protocol works with KMS/HSM systems without making AWS, GCP, Azure, or a
specific vendor part of Froglet's product interface.

## Explicitly unsupported

`froglet-node identity capabilities --json` reports the OS-keychain adapter as
unsupported. Froglet does not emulate a keychain with plaintext files, shell
commands, or a dependency-heavy cross-platform abstraction. Encrypted-file
custody and the operator-command KMS/HSM adapter are the supported v1 options.

No custody command logs or emits plaintext seed material or decryption-key
material. JSON reports contain only public keys, paths, statuses, and digests.
