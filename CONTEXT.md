# Froglet Context

Froglet is a protocol and node for signed resource deals between bots. This context names the product concepts used when discussing architecture so future reviews do not drift into generic infrastructure language.

## Language

**Kernel**:
The stable Froglet v1 economic contract: signed artifacts, canonical hashes, signatures, artifact relationships, deal states, and settlement semantics.
_Avoid_: Core protocol, wire layer

**Artifact Chain**:
The ordered evidence chain that proves a Froglet interaction: Descriptor, Offer, Quote, Deal, optional InvoiceBundle, and Receipt linked by hashes.
_Avoid_: Transaction log, workflow, request chain

**Froglet Node**:
A process that can act as provider or requester per deal, exposing provider and runtime surfaces over configured transports.
_Avoid_: Server, service

**Provider**:
The role in a deal that signs the Descriptor, Offer, Quote, and Receipt and executes the requested resource.
_Avoid_: Seller, worker, executor

**Requester**:
The role in a deal that asks for a resource, signs the Deal, and may provide or release settlement material.
_Avoid_: Buyer, client, caller

**Service Manifest**:
The metadata and binding information that lets an operator turn a named or data service invocation into a Froglet workload.
_Avoid_: Tool definition, plugin manifest

**Publication**:
The agent-controlled process that turns a Service Manifest plus workload or data into an externally verified, optionally listed Froglet offering.
_Avoid_: Deploy, upload

**Publication Revision**:
An immutable, content-addressed version of a publication whose workload, configuration, and signed offer can be verified and activated or rolled back as one unit.
_Avoid_: Latest version, mutable deployment

**Reachability Lease**:
A renewable, identity-authenticated claim to a public endpoint that forwards traffic to a Froglet Node without changing Kernel semantics.
_Avoid_: DNS record, tunnel URL

**Managed Deployment**:
An operator-run Froglet workload revision with explicit compute, storage, secret, health, and lifecycle requirements, independent of the infrastructure provider that satisfies them.
_Avoid_: AWS stack, cloud service

**Release Bundle**:
The signed manifest that pins every binary, image, agent integration, checksum, and compatibility fact shipped as one Froglet release.
_Avoid_: Latest tag, download list
