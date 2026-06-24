// ═══════ Demo step data and TypeScript interfaces ═══════
// Extracted from demo.astro inline script (Requirements 1.3, 11.1)

export interface BoardNode {
  id: string;
  label: string;
  sub?: string;
  x: number;  // 0..1 normalized
  y: number;  // 0..1 normalized
  highlight?: boolean;
  small?: boolean;
  lifeline?: number;  // 0..1 normalized Y where the lifeline ends
}

export interface BoardArrow {
  from: string;
  to: string;
  label?: string;
  bidi?: boolean;
  style?: 'dashed';
  y?: number;
}

export interface BoardNote {
  text: string;
  x: number;
  y: number;
  size?: number;
  color?: 'accent' | 'warn' | 'muted';
}

export interface TerminalLine {
  p?: string;  // prompt (if present, `c` is the command to type)
  c?: string;  // command text
  o?: string;  // output line (if no `p`)
}

export interface Step {
  t: string;           // title
  d: string;           // description
  link: string;        // learn more URL
  note?: string;       // annotation HTML
  board: {
    nodes: BoardNode[];
    arrows: BoardArrow[];
    notes: BoardNote[];
  };
  term: TerminalLine[];
}

export const STEPS: Step[] = [
{
  t:"The network",
  d:`Three roles, one protocol. Providers sell resources. Requesters buy them. Marketplaces help them find each other.`,
  link:"/architecture/overview/",
  note:`Every node can play <strong>multiple roles</strong>. A marketplace is itself a provider that sells search. <span class="hl">There are no special node classes.</span>`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.18,y:.35},
      {id:'fr',label:'F^R',sub:'requester',x:.82,y:.35},
      {id:'m',label:'M',sub:'marketplace',x:.50,y:.72},
    ],
    arrows:[
      {from:'fp',to:'fr',label:'deal',bidi:true},
      {from:'fp',to:'m',label:'register'},
      {from:'fr',to:'m',label:'discover'},
    ],
    notes:[
      {text:'Provider Froglet -> F^P',x:.07,y:.90,size:16},
      {text:'Requester -> F^R,  Marketplace -> M',x:.07,y:.95,size:16},
    ],
  },
  term:[
    {p:"~",c:"FROGLET_PAYMENT_BACKEND=none FROGLET_PUBLISH_DEMO_SERVICES=1 cargo run --bin froglet-node"},
    {o:"<span class='tm'>local assumption: 127.0.0.1:8080 and :8081 are free</span>"},
    {o:"<span class='to'>STARTUP OUTPUT (ABRIDGED)</span>"},
    {o:"Local Runtime API: http://127.0.0.1:8081"},
    {o:"Local API Gateway: http://127.0.0.1:8080"},
    {o:"Node is now online and accepting traffic."},
  ],
},
{
  t:"The problem",
  d:`An agent needs compute from a provider across the world. Four questions need answers.`,
  link:"/learn/introduction/",
  note:`The internet was built for humans to click buttons. Agents need infrastructure to transact <strong>autonomously</strong>. Froglet answers what the protocol can prove with <span class="hl">signed artifacts, receipts, and settlement state</span>.`,
  board:{
    nodes:[
      {id:'fr',label:'F^R',sub:'requester',x:.20,y:.40},
      {id:'fp',label:'F^P',sub:'provider',x:.80,y:.40},
    ],
    arrows:[
      {from:'fr',to:'fp',label:'?',style:'dashed'},
    ],
    notes:[
      {text:'FIND    how does F^R discover F^P?',x:.15,y:.65,size:14,color:'muted'},
      {text:'TRUST   how does F^R know F^P is reliable?',x:.15,y:.72,size:14,color:'muted'},
      {text:'PAY     how does F^R pay without a bank?',x:.15,y:.79,size:14,color:'muted'},
      {text:'PROVE   what can F^R audit after execution?',x:.15,y:.86,size:14,color:'muted'},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/node/identity"},
    {o:"<span class='to'>IDENTITY OUTPUT (ABRIDGED)</span>"},
    {o:'<span class=\'th\'>{"node_id":"&lt;64-hex x-only key&gt;","public_key":"&lt;same key&gt;"}</span>'},
  ],
},
{
  t:"Identity by keypair",
  d:`Generate a keypair locally. Your public key IS your identity. No registration.`,
  link:"/learn/identity/",
  note:`<strong>secp256k1</strong> elliptic curve. Private key signs. Public key verifies. <span class="hl">Mathematically irreversible.</span> Your key follows you across providers and marketplaces.`,
  board:{
    nodes:[
      {id:'f',label:'F',sub:'new node',x:.50,y:.35,highlight:true},
    ],
    arrows:[],
    notes:[
      {text:'1. generate keypair (sk, pk)',x:.25,y:.58,size:15},
      {text:'2. pk = node identity',x:.25,y:.66,size:15},
      {text:'3. sk signs all artifacts',x:.25,y:.74,size:15},
      {text:'No registration. No authority.',x:.25,y:.88,size:13,color:'accent'},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/node/identity"},
    {o:"<span class='to'>IDENTITY OUTPUT (ABRIDGED)</span>"},
    {o:'{"node_id":"&lt;64-hex secp256k1 x-only public key&gt;",'},
    {o:' "public_key":"&lt;same 64-hex public key&gt;"}'},
  ],
},
{
  t:"Signed artifacts",
  d:`Every interaction produces signed, hash-linked evidence. Six artifact types form a chain.`,
  link:"/learn/deal-flow/",
  note:`<strong>RFC 8785 canonical JSON</strong> ensures identical bytes. <span class="hl">SHA-256</span> hashed, <span class="hl">BIP340 Schnorr</span> signed. Each artifact references the previous by hash.`,
  board:{
    nodes:[
      {id:'a1',label:'descriptor',x:.08,y:.40,small:true},
      {id:'a2',label:'offer',x:.24,y:.40,small:true},
      {id:'a3',label:'quote',x:.40,y:.40,small:true},
      {id:'a4',label:'deal',x:.56,y:.40,small:true},
      {id:'a5',label:'invoice_bundle',x:.73,y:.40,small:true},
      {id:'a6',label:'receipt',x:.92,y:.40,small:true},
    ],
    arrows:[
      {from:'a1',to:'a2',label:'H(a1)'},
      {from:'a2',to:'a3',label:'H(a2)'},
      {from:'a3',to:'a4',label:'H(a3)'},
      {from:'a4',to:'a5',label:'H(a4)'},
      {from:'a5',to:'a6',label:'H(a5)'},
    ],
    notes:[
      {text:'provider signs',x:.04,y:.28,size:12,color:'muted'},
      {text:'provider signs',x:.20,y:.28,size:12,color:'muted'},
      {text:'provider signs',x:.36,y:.28,size:12,color:'muted'},
      {text:'requester signs',x:.52,y:.28,size:12,color:'muted'},
      {text:'provider signs',x:.69,y:.28,size:12,color:'muted'},
      {text:'provider signs',x:.88,y:.28,size:12,color:'muted'},
      {text:'Each linked by SHA-256 hash. Tamper with one, chain breaks.',x:.12,y:.62,size:14,color:'accent'},
    ],
  },
  term:[
    {o:"<span class='to'>SIGNED ENVELOPE SHAPE (ABRIDGED)</span>"},
    {o:'{"artifact_type":"quote",'},
    {o:' "payload_hash":"a3f19e2b...7d4c",'},
    {o:' "signature":"&lt;64-byte BIP340 Schnorr signature hex&gt;"}'},
  ],
},
{
  t:"Discovery",
  d:`A marketplace is optional. It indexes signed provider feeds so requesters can search.`,
  link:"/marketplace/overview/",
  note:`The marketplace is not a protocol root of truth. It consumes <strong>signed descriptors and offers</strong>; kernel quote, deal, receipt, and settlement semantics stay the same with or without it.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.15,y:.28},
      {id:'m',label:'M',sub:'marketplace',x:.55,y:.60},
      {id:'fr',label:'F^R',sub:'requester',x:.85,y:.28},
    ],
    arrows:[
      {from:'fp',to:'m',label:'publish feed'},
      {from:'fr',to:'m',label:'discover'},
    ],
    notes:[
      {text:'M does:',x:.42,y:.78,size:13},
      {text:'1. verify signed descriptor',x:.42,y:.84,size:13},
      {text:'2. index bound offers',x:.42,y:.90,size:13},
      {text:'3. return provider URLs',x:.42,y:.96,size:13},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/provider/services"},
    {o:"<span class='to'>SERVICES OUTPUT (ABRIDGED)</span>"},
    {o:'<span class=\'ts\'>{"services":[{"service_id":"demo.add","price_sats":0}, ...]}</span>'},
  ],
},
{
  t:"The deal",
  d:`Search, quote, commit, execute, receipt. The complete lifecycle of one interaction.`,
  link:"/learn/deal-flow/",
  note:`The requester searches, picks a provider, gets a quote, signs the deal, workload executes, receipt returned. <span class="hl">Every step is hash-linked.</span>`,
  board:{
    nodes:[
      {id:'fr',label:'F^R',sub:'requester',x:.18,y:.12,lifeline:.72},
      {id:'fp',label:'F^P',sub:'provider',x:.82,y:.12,lifeline:.72},
    ],
    arrows:[
      {from:'fr',to:'fp',label:'1. request quote',y:.28},
      {from:'fp',to:'fr',label:'2. signed quote',y:.38},
      {from:'fr',to:'fp',label:'3. signed deal',y:.50},
      {from:'fp',to:'fr',label:'4. execute + receipt',y:.62},
    ],
    notes:[
      {text:'base fee locks on acceptance',x:.28,y:.80,size:13,color:'muted'},
      {text:'success fee settles on success',x:.28,y:.87,size:13,color:'muted'},
    ],
  },
  term:[
    {o:"<span class='to'>RUNTIME DEAL REQUEST (ABRIDGED)</span>"},
    {p:"~",c:"curl -H 'Authorization: Bearer &lt;runtime-token&gt;' -X POST http://127.0.0.1:8081/v1/runtime/deals --data '&lt;full demo.add workload&gt;'"},
    {o:"<span class='to'>DEAL OUTPUT (ABRIDGED)</span>"},
    {o:'{"deal":{"status":"succeeded",'},
    {o:' "result":{"sum":12},'},
    {o:' "receipt":{"payload":{"execution_state":"succeeded","settlement_state":"none"}}}}'},
  ],
},
{
  t:"Settlement",
  d:`Two fees protect both sides. Base fee prevents free-riding. Success fee aligns incentives.`,
  link:"/learn/settlement/",
  note:`<strong>Base fee</strong> locks via Lightning HTLC on deal acceptance. <strong>Success fee</strong> settles only on execution success. Provider profit <span class="hl">increases with successful completion</span>. No platform escrow account is needed.`,
  board:{
    nodes:[
      {id:'fr',label:'F^R',sub:'requester',x:.18,y:.12,lifeline:.72},
      {id:'fp',label:'F^P',sub:'provider',x:.82,y:.12,lifeline:.72},
    ],
    arrows:[
      {from:'fr',to:'fp',label:'1. accept: base fee locks',y:.30},
      {from:'fr',to:'fp',label:'2. success hold accepted',y:.43},
      {from:'fp',to:'fr',label:'3. receipt settles success',y:.58},
    ],
    notes:[
      {text:'Outcome      Requester      Provider',x:.18,y:.78,size:16,color:'muted'},
      {text:'success      -8 sat         +8 sat',x:.18,y:.84,size:16,color:'accent'},
      {text:'failure      -3 sat         +3 sat',x:.18,y:.90,size:16,color:'warn'},
      {text:'no deal       0              0',x:.18,y:.96,size:16,color:'muted'},
    ],
  },
  term:[
    {o:"<span class='to'>SETTLEMENT TERMS (EXAMPLE)</span>"},
    {o:"  base_fee_msat:    3000  <span class='tm'>locks on deal acceptance</span>"},
    {o:"  success_fee_msat: 5000  <span class='tm'>settles on execution success</span>"},
    {o:"  method: lightning.base_fee_plus_success_fee.v1"},
  ],
},
{
  t:"Trust signals",
  d:`Trust today is evidence, not magic: receipts, settled value, attestation, and marketplace policy.`,
  link:"/learn/economics/",
  note:`A signed receipt proves attribution and the committed result hash. It <strong>does not prove result correctness by itself</strong>. Stake-backed identity is roadmap, not a live guarantee.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.20,y:.30},
      {id:'m',label:'M',sub:'marketplace',x:.65,y:.30},
    ],
    arrows:[
      {from:'fp',to:'m',label:'signed receipts'},
    ],
    notes:[
      {text:'Today: receipt-derived history',x:.15,y:.55,size:15,color:'accent'},
      {text:'Today: domain / OAuth attestation',x:.15,y:.64,size:14},
      {text:'Today: operator arbiter policy',x:.15,y:.73,size:14},
      {text:'Roadmap: settlement-backed stake',x:.15,y:.84,size:14,color:'muted'},
      {text:'Correctness still needs checks',x:.15,y:.93,size:14,color:'warn'},
    ],
  },
  term:[
    {o:"<span class='to'>TRUST MODEL SUMMARY</span>"},
    {o:"  live: signed receipts + attestation + arbiter policy"},
    {o:"  not live: stake-backed slashing"},
    {o:"  receipt: attribution, not automatic correctness"},
  ],
},
{
  t:"The pluggable stack",
  d:`Core stays the same. Runtime, payment, and transport are adapter layers.`,
  link:"/architecture/overview/",
  note:`The signed envelope and verifier rules stay the same. Adapter-specific fields change, but the kernel artifact chain remains the common proof surface.`,
  board:{
    nodes:[
      {id:'core',label:'core',sub:'protocol kernel',x:.50,y:.22,highlight:true},
    ],
    arrows:[],
    notes:[
      {text:'EXECUTION',x:.08,y:.42,size:14,color:'accent'},
      {text:'  wasm  /  python  /  container  /  builtin  /  gpu capability',x:.08,y:.50,size:13},
      {text:'SETTLEMENT',x:.08,y:.62,size:14,color:'accent'},
      {text:'  none  /  lightning  /  stripe  /  x402 adapters',x:.08,y:.70,size:13},
      {text:'TRANSPORT',x:.08,y:.82,size:14,color:'accent'},
      {text:'  clearnet  /  tor support; no hosted Bluetooth proof',x:.08,y:.90,size:13},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/node/capabilities"},
    {o:"<span class='to'>CAPABILITIES OUTPUT (ABRIDGED)</span>"},
    {o:'{"execution":{"wasm":{"enabled":true},"gpu":{"enabled":false}},'},
    {o:' "payments":{"backend":"none","accepted_payment_methods":[]},'},
    {o:' "transports":{"clearnet":{"enabled":true},"tor":{"enabled":false}}}'},
  ],
},
{
  t:"Agents and integrations",
  d:`AI agents use froglet through MCP, OpenClaw, NemoClaw, or raw HTTP.`,
  link:"/learn/introduction/",
  note:`<strong>OpenClaw:</strong> Claude tool. <strong>NemoClaw:</strong> local LLM. <strong>MCP Server:</strong> Claude Code, Cursor, Windsurf. The kernel has signed deals; local and hosted API surfaces still use normal auth tokens.`,
  board:{
    nodes:[
      {id:'agent',label:'Agent',sub:'Claude / GPT / Llama',x:.18,y:.35},
      {id:'fr',label:'F^R',sub:'requester',x:.50,y:.35},
      {id:'fp',label:'F^P',sub:'provider',x:.82,y:.35},
    ],
    arrows:[
      {from:'agent',to:'fr',label:'MCP / OpenClaw'},
      {from:'fr',to:'fp',label:'deal'},
    ],
    notes:[
      {text:'Integrations:',x:.12,y:.62,size:14,color:'accent'},
      {text:'  OpenClaw      Claude tool interface',x:.12,y:.70,size:13},
      {text:'  NemoClaw      local LLM agent',x:.12,y:.77,size:13},
      {text:'  MCP Server    Claude Code, Cursor, Windsurf',x:.12,y:.84,size:13},
      {text:'  HTTP          any client with normal API auth',x:.12,y:.91,size:13},
    ],
  },
  term:[
    {p:"~",c:"npx froglet-mcp"},
    {o:"<span class='to'>INTEGRATION SUMMARY (NOT CLI STDOUT)</span>"},
    {o:"starts MCP server over stdio for local or self-hosted nodes"},
    {o:"agent tools still call the same runtime/provider HTTP APIs"},
  ],
},
{
  t:"The incentive boundary",
  d:`Identity is free. Receipts make outcomes attributable. Correctness still requires checks.`,
  link:"/learn/economics/",
  note:`The protocol does not make trust magical. It makes claims signed, hash-linked, and auditable. Stake can strengthen this later, but only with detection and adjudication.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.18,y:.25},
      {id:'fr',label:'F^R',sub:'requester',x:.82,y:.25},
      {id:'m',label:'M',sub:'marketplace',x:.50,y:.55},
    ],
    arrows:[
      {from:'fp',to:'fr',label:'deal',bidi:true},
      {from:'fp',to:'m',label:'publish evidence'},
      {from:'fr',to:'m',label:'discover'},
    ],
    notes:[
      {text:'identity      free keypair',x:.12,y:.76,size:13},
      {text:'receipt       signed outcome',x:.12,y:.83,size:13},
      {text:'correctness   re-run, inspect, or verify',x:.12,y:.90,size:13,color:'warn'},
      {text:'stake         roadmap trust signal',x:.12,y:.97,size:14,color:'accent'},
    ],
  },
  term:[
    {o:"<span class='to'>WHAT IS PROVEN TODAY (SUMMARY)</span>"},
    {o:"<span class='ts'>signed</span>     who committed to which bytes"},
    {o:"<span class='ts'>receipt</span>    outcome + result hash + terminal state"},
    {o:"<span class='te'>not proven</span> correctness without checking"},
    {o:""},
    {o:"<span class='ts'>best practice: small deals + evidence + re-checks</span>"},
  ],
},
];
