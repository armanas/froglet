# Froglet and Nostr

Status: non-normative supporting document

Nostr is an optional publication and discovery fabric for Froglet.
It is not the source of canonical economic state.

## 1. Role of Nostr

Nostr may be used to publish:

- descriptor summaries
- offer summaries
- artifact hashes
- endpoint hints
- receipt summaries
- signed curated peer lists

Requesters must still verify the underlying Froglet artifacts.

## 2. Publication Identity

Providers should sign Nostr summaries with a distinct linked Nostr publication key rather than the Froglet root key.

That publication key should appear in `Descriptor.payload.linked_identities[]` with:

- `identity_kind = nostr`
- scope including `publication.nostr`

The exact linkage challenge format is defined in [`../SPEC.md`](../SPEC.md).

## 3. Suggested Summary Events

Current adapter guidance is:

- descriptor summaries: addressable kind `30390`, with `d = provider_id`
- offer summaries: addressable kind `30391`, with `d = offer_id`
- receipt summaries: append-only kind `1390`

Descriptor and offer summaries should include an `alt` tag.
Offer summaries should include an `expiration` tag whenever the underlying offer expires.

These are adapter conventions, not kernel commitments.

## 4. Relay Policy

Relay choice, relay `AUTH`, retry behavior, backoff, read/write separation, and relay-list policy belong to the external Nostr adapter.

The core Froglet node should not become responsible for:

- relay selection
- relay authentication policy
- retry loops
- relay reputation
- relay fanout strategy

## 5. Verification Boundary

A Nostr summary is useful only if a verifier can trace it back to:

- a linked publication key in the descriptor
- the referenced Froglet artifact hash
- the underlying signed Froglet artifact itself

Nostr improves dissemination.
It does not replace the kernel evidence chain.
