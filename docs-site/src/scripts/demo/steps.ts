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
    {p:"~",c:"FROGLET_PAYMENT_BACKEND=none cargo run --bin froglet-node"},
    {o:"<span class='ts'>froglet-node</span> listening on 127.0.0.1:8080"},
  ],
},
{
  t:"The problem",
  d:`An agent needs compute from a provider across the world. Four questions need answers.`,
  link:"/learn/introduction/",
  note:`The internet was built for humans to click buttons. Agents need infrastructure to transact <strong>autonomously</strong>. Froglet answers all four with <span class="hl">cryptography and staked reputation</span>.`,
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
      {text:'PROVE   how does F^R verify execution?',x:.15,y:.86,size:14,color:'muted'},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/node/identity"},
    {o:'<span class=\'th\'>{"node_id":"7f3a2b91","public_key":"027f3a2b..."}</span>'},
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
    {o:'{"node_id":"7f3a2b91e8c4d156",'},
    {o:' "public_key":"027f3a2b91e8c4d1563a0e9f8b4c7d2a..."}'},
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
    {p:"~",c:"curl -X POST http://127.0.0.1:8080/v1/provider/quotes -H 'Content-Type: application/json' -d '{\"offer_id\":\"execute.compute.generic\"}'"},
    {o:'{"artifact_type":"quote",'},
    {o:' "payload_hash":"a3f19e2b...7d4c",'},
    {o:' "signature":"3045022100..."}'},
  ],
},
{
  t:"Discovery",
  d:`A marketplace is just another froglet node. It sells search as a service, using the same deal flow.`,
  link:"/marketplace/overview/",
  note:`Registration is a <strong>standard froglet deal</strong>. After registering, the provider <span class="hl">stakes</span> non-refundable value into their identity.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.15,y:.28},
      {id:'m',label:'M',sub:'marketplace',x:.55,y:.60},
      {id:'fr',label:'F^R',sub:'requester',x:.85,y:.28},
    ],
    arrows:[
      {from:'fp',to:'m',label:'register + stake'},
      {from:'fr',to:'m',label:'discover'},
    ],
    notes:[
      {text:'M does:',x:.42,y:.78,size:13},
      {text:'1. index new service',x:.42,y:.84,size:13},
      {text:'2. record stake',x:.42,y:.90,size:13},
      {text:'3. generate receipt',x:.42,y:.96,size:13},
    ],
  },
  term:[
    {p:"~",c:"curl -X POST http://127.0.0.1:8080/v1/provider/deals -H 'Content-Type: application/json' -d '{\"offer_id\":\"marketplace.register\"}'"},
    {o:'<span class=\'ts\'>{"status":"registered","provider_id":"7f3a2b91"}</span>'},
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
    {p:"~",c:"curl -X POST http://127.0.0.1:8080/v1/provider/deals -H 'Content-Type: application/json' -d '{\"quote_hash\":\"a3f19e2b...\",\"signed_deal\":{...}}'"},
    {o:'{"deal_id":"b4a2e7f1",'},
    {o:' "deal_state":"accepted",'},
    {o:' "execution_state":"succeeded",'},
    {o:' "result_hash":"sha256:9f1a4b2c...",'},
    {o:' "settlement_state":"settled"}'},
  ],
},
{
  t:"Settlement",
  d:`Two fees protect both sides. Base fee prevents free-riding. Success fee aligns incentives.`,
  link:"/learn/settlement/",
  note:`<strong>Base fee</strong> locks via Lightning HTLC on deal acceptance. <strong>Success fee</strong> settles only on execution success. Provider profit <span class="hl">increases with quality</span>. No escrow needed.`,
  board:{
    nodes:[
      {id:'fr',label:'F^R',sub:'requester',x:.20,y:.30},
      {id:'fp',label:'F^P',sub:'provider',x:.80,y:.30},
    ],
    arrows:[
      {from:'fr',to:'fp',label:'base fee'},
      {from:'fr',to:'fp',label:'success fee',y:.54},
    ],
    notes:[
      {text:'accept locks',x:.39,y:.40,size:17,color:'accent'},
      {text:'success pays',x:.39,y:.62,size:17,color:'accent'},
      {text:'Outcome      Requester      Provider',x:.18,y:.76,size:16,color:'muted'},
      {text:'success      -8 sat         +8 sat',x:.18,y:.82,size:16,color:'accent'},
      {text:'failure      -3 sat         +3 sat',x:.18,y:.88,size:16,color:'warn'},
      {text:'no deal       0              0',x:.18,y:.94,size:16,color:'muted'},
    ],
  },
  term:[
    {o:"<span class='to'>SETTLEMENT</span>"},
    {o:"  base_fee_msat:    3000  <span class='tm'>locks on deal acceptance</span>"},
    {o:"  success_fee_msat: 5000  <span class='tm'>settles on execution success</span>"},
    {o:"  method: lightning.base_fee_plus_success_fee.v1"},
  ],
},
{
  t:"Staked reputation",
  d:`Trust is the total non-refundable value staked into an identity. Not ratings. Not deal history. Real money at risk.`,
  link:"/learn/economics/",
  note:`Stake is <span class="hl">non-refundable and public</span>. Cheating destroys the identity. Self-dealing cannot inflate T because only <strong>real payments into the stake</strong> change it.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.20,y:.30},
      {id:'m',label:'M',sub:'marketplace',x:.65,y:.30},
    ],
    arrows:[
      {from:'fp',to:'m',label:'marketplace.stake'},
    ],
    notes:[
      {text:'T(F^P) = total_staked_msat',x:.15,y:.55,size:16,color:'accent'},
      {text:'Safety ratio = stake / deal_value',x:.15,y:.65,size:14},
      {text:'  8 sat deal:   5,268x   safe',x:.15,y:.74,size:13,color:'muted'},
      {text:' 100 sat deal:    421x   safe',x:.15,y:.81,size:13,color:'muted'},
      {text:'1000 sat deal:     42x   safe',x:.15,y:.88,size:13,color:'muted'},
    ],
  },
  term:[
    {p:"~",c:"curl -X POST http://127.0.0.1:8080/v1/provider/deals -H 'Content-Type: application/json' -d '{\"offer_id\":\"marketplace.stake\",\"input\":{\"provider_id\":\"7f3a2b91\",\"amount_msat\":42150}}'"},
    {o:'{"provider_id":"7f3a2b91",'},
    {o:' "total_staked_msat":42150,'},
    {o:' "status":"staked"}'},
  ],
},
{
  t:"The pluggable stack",
  d:`Core stays the same. Runtime, payment, transport \u2014 every layer is swappable.`,
  link:"/architecture/overview/",
  note:`The signed artifact chain is <span class="hl">identical regardless of stack</span>. Swap any layer without touching the protocol.`,
  board:{
    nodes:[
      {id:'core',label:'core',sub:'protocol kernel',x:.50,y:.22,highlight:true},
    ],
    arrows:[],
    notes:[
      {text:'EXECUTION',x:.08,y:.42,size:14,color:'accent'},
      {text:'  wasm  /  python  /  container  /  gpu (coming)',x:.08,y:.50,size:13},
      {text:'SETTLEMENT',x:.08,y:.62,size:14,color:'accent'},
      {text:'  none  /  lightning  /  fiat (coming)',x:.08,y:.70,size:13},
      {text:'TRANSPORT',x:.08,y:.82,size:14,color:'accent'},
      {text:'  clearnet  /  tor  /  bluetooth (coming)',x:.08,y:.90,size:13},
    ],
  },
  term:[
    {p:"~",c:"curl http://127.0.0.1:8080/v1/node/capabilities"},
    {o:'{"execution":["wasm","python","container","builtin"],'},
    {o:' "settlement":["none","lightning"],'},
    {o:' "transport":["clearnet"]}'},
  ],
},
{
  t:"Agents and integrations",
  d:`AI agents use froglet through MCP, OpenClaw, NemoClaw, or raw HTTP.`,
  link:"/learn/introduction/",
  note:`<strong>OpenClaw:</strong> Claude tool. <strong>NemoClaw:</strong> local LLM. <strong>MCP Server:</strong> Claude Code, Cursor, Windsurf. <span class="hl">No API keys. No platforms. Just signed deals.</span>`,
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
      {text:'  MCP Server    Claude Code, Cursor, VS Code',x:.12,y:.84,size:13},
      {text:'  HTTP          any client, no SDK needed',x:.12,y:.91,size:13},
    ],
  },
  term:[
    {p:"~",c:"curl -X POST http://127.0.0.1:8081/v1/runtime/deals -H 'Content-Type: application/json' -d '{\"offer_id\":\"execute.compute.generic\",\"provider\":\"7f3a2b91\"}'"},
    {o:'{"deal_id":"7e2fb1d4",'},
    {o:' "execution_state":"succeeded",'},
    {o:' "result_hash":"sha256:c4d2a6e1..."}'},
  ],
},
{
  t:"The equilibrium",
  d:`Identity is free. Stake costs real sats. Cheating burns the stake. <strong>Honesty is the Nash equilibrium.</strong>`,
  link:"/learn/economics/",
  note:`The protocol does not trust anyone. It makes honesty the cheapest option. Stake >> deal value means cheating is <span class="hl">always irrational</span>.`,
  board:{
    nodes:[
      {id:'fp',label:'F^P',sub:'provider',x:.18,y:.25},
      {id:'fr',label:'F^R',sub:'requester',x:.82,y:.25},
      {id:'m',label:'M',sub:'marketplace',x:.50,y:.55},
    ],
    arrows:[
      {from:'fp',to:'fr',label:'deal',bidi:true},
      {from:'fp',to:'m',label:'register + stake'},
      {from:'fr',to:'m',label:'discover'},
    ],
    notes:[
      {text:'identity    free       generate keypair',x:.12,y:.76,size:13},
      {text:'stake       expensive  real sats, non-refundable',x:.12,y:.83,size:13},
      {text:'cheating    destroys stake investment',x:.12,y:.90,size:13,color:'warn'},
      {text:'when S >> V: honesty is dominant',x:.12,y:.97,size:14,color:'accent'},
    ],
  },
  term:[
    {o:"<span class='to'>STRATEGY   THIS DEAL   STAKE       FUTURE</span>"},
    {o:"<span class='ts'>honest</span>     +8 sat      preserved   continues"},
    {o:"<span class='te'>cheat</span>      +3 sat      lost (S)    zero"},
    {o:""},
    {o:"<span class='ts'>when S >> V: honesty is dominant</span>"},
  ],
},
];
