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
